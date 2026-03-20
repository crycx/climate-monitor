import { Injectable } from '@angular/core';
import { CanActivate, CanActivateChild, Router, UrlTree } from '@angular/router';
import { Observable, map } from 'rxjs';
import { MatSnackBar } from '@angular/material/snack-bar';
import { AuthService } from './auth.service';
import { Snackbar } from '../custom-components/snackbar/snackbar';

@Injectable({ providedIn: 'root' })
export class AdminGuard implements CanActivate, CanActivateChild {
  constructor(
    private auth: AuthService,
    private router: Router,
    private snack: MatSnackBar
  ) {}

  canActivate(): Observable<boolean | UrlTree> {
    return this.auth.me().pipe(
      map((loggedIn) => {
        if (!loggedIn) return this.router.parseUrl('/login');

        if (this.auth.user?.is_admin) return true;

        this.snack.openFromComponent(Snackbar, {
          data: {
            text: 'No rights to see this page',
            icon: 'error',
            iconColor: 'red',
            durationMs: 3000,
            dismiss: 'OK',
          },
        });

        return this.router.parseUrl('/');
      })
    );
  }

  canActivateChild(): Observable<boolean | UrlTree> {
    return this.canActivate();
  }

}
