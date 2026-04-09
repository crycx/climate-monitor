import { ApplicationRef, Component, NgZone, OnInit, signal } from '@angular/core';
import { MatIconRegistry } from '@angular/material/icon';
import { MatSnackBar } from '@angular/material/snack-bar'
import { SwUpdate } from '@angular/service-worker';
import { filter } from 'rxjs/internal/operators/filter';
import { first } from 'rxjs/internal/operators/first';

@Component({
  selector: 'app-root',
  templateUrl: './app.html',
  standalone: false,
  styleUrl: './app.css'
})
export class App implements OnInit {
  protected readonly title = signal('climate-monitor');
  private updateShown = false;

  constructor(private matIconReg: MatIconRegistry,private sw: SwUpdate,private snack: MatSnackBar,private appRef: ApplicationRef,private zone: NgZone,) {
    if (!this.sw.isEnabled) return;

    // Logi kõik update eventid
    // this.sw.versionUpdates.subscribe(e => console.log('[SW] versionUpdates:', e));

    // Kui uus versioon on valmis, näita snackbari
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


}
