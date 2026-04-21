import { ApplicationRef, Component, NgZone, OnInit, signal } from '@angular/core';
import { MatIconRegistry } from '@angular/material/icon';
import { MatSnackBar } from '@angular/material/snack-bar';
import { NavigationEnd, Router } from '@angular/router';
import { SwUpdate } from '@angular/service-worker';
import { Observable, filter, first } from 'rxjs';
import { AuthService, AuthUser } from './auth/auth.service';

type NavItem = {
  label: string;
  route: string;
  icon: string;
  adminOnly?: boolean;
  exact?: boolean;
};

@Component({
  selector: 'app-root',
  templateUrl: './app.html',
  standalone: false,
  styleUrl: './app.css'
})
export class App implements OnInit {
  protected readonly title = signal('climate-monitor');
  protected readonly currentUrl = signal('');
  protected readonly sidebarOpen = signal(false);
  protected readonly sidebarHidden = signal(false);
  protected readonly user$: Observable<AuthUser | null>;
  protected readonly navItems: NavItem[] = [
    { label: 'Overview', route: '/', icon: 'dashboard', exact: true },
    { label: 'Devices', route: '/devices', icon: 'memory' },
    { label: 'Locations', route: '/locations', icon: 'place' },
    { label: 'Users', route: '/users', icon: 'group', adminOnly: true },
  ];
  private updateShown = false;

  constructor(
    private matIconReg: MatIconRegistry,
    private sw: SwUpdate,
    private snack: MatSnackBar,
    private appRef: ApplicationRef,
    private zone: NgZone,
    private auth: AuthService,
    private router: Router
  ) {
    this.user$ = this.auth.user$;
    this.currentUrl.set(this.router.url);

    // Logi kõik update eventid
    // this.sw.versionUpdates.subscribe(e => console.log('[SW] versionUpdates:', e));
    this.router.events
      .pipe(filter((event): event is NavigationEnd => event instanceof NavigationEnd))
      .subscribe((event) => {
        this.currentUrl.set(event.urlAfterRedirects);
        this.sidebarOpen.set(false);
      });

    // Kui uus versioon on valmis, näita snackbari
    if (!this.sw.isEnabled) return;
    this.sw.versionUpdates
      .pipe(filter(e => e.type === 'VERSION_READY'))
      .subscribe(() => this.showUpdateBar());

    // Oota kuni Angular on "stable", siis alusta checke
    this.appRef.isStable.pipe(first(stable => stable)).subscribe(() => {

      // SW registreerimine võib võtta hetke — anna 2s enne esimest checke
      setTimeout(() => {
        this.safeCheck();
        setInterval(() => this.safeCheck(), 60_000);
      }, 2000);
    });

  }

  ngOnInit(): void {
    this.matIconReg.setDefaultFontSetClass('material-symbols-outlined');
    // this.auth.me().subscribe((loggedIn) => {
    //   if (loggedIn) this.auth.startMePolling(60_000);
    // });
  }

  protected showShell(user: AuthUser | null): boolean {
    return !!user && !this.isPublicRoute(this.currentUrl());
  }

  protected visibleNavItems(user: AuthUser): NavItem[] {
    return this.navItems.filter((item) => !item.adminOnly || user.is_admin);
  }

  protected isActive(item: NavItem): boolean {
    const url = this.currentUrl();
    if (item.exact) return url === item.route;
    return url === item.route || url.startsWith(`${item.route}/`);
  }

  protected toggleSidebar(): void {
    this.sidebarOpen.update((v) => !v);
  }

  protected closeSidebar(): void {
    this.sidebarOpen.set(false);
  }

  protected toggleSidebarHidden(): void {
    this.sidebarHidden.update((v) => !v);
  }

  protected logout(): void {
    this.auth.logout().subscribe();
  }

  private safeCheck(): void {
    this.zone.runOutsideAngular(async () => {
      try {
        await this.withTimeout(this.sw.checkForUpdate(), 8000);
      } catch (e) {
        console.error('[SW] check error/timeout:', e);
      }
    });
  }

  private withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(`checkForUpdate timeout after ${ms}ms`)), ms);
      p.then(v => { clearTimeout(t); resolve(v); })
       .catch(err => { clearTimeout(t); reject(err); });
    });
  }

  private showUpdateBar() {
    if (this.updateShown) return;
    this.updateShown = true;

    // SnackBar tuleb ainult siis, kui VERSION_READY tuli
    const ref = this.snack.open('New version available', 'Update', {
      duration: 0,
      horizontalPosition: 'center',
      verticalPosition: 'top',
    });

    ref.onAction().subscribe(() => {
      this.sw.activateUpdate().then(() => location.reload());
    });

    ref.afterDismissed().subscribe(() => {
      // siia võid hiljem lisada "ära tüüta uuesti" loogika
    });
  }

  private isPublicRoute(url: string): boolean {
    return (
      url === '/login' ||
      url === '/register' ||
      url === '/forgot-password' ||
      url.startsWith('/reset-password')
    );
  }
}
