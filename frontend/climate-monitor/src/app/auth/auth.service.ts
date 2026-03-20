import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { BehaviorSubject, catchError, map, Observable, of, Subscription, switchMap, tap, timer } from 'rxjs';
import { environment } from '../../environments/environment';
import { Router } from '@angular/router';

export type AuthUser = {
  id: number;
  email: string;
  username: string;
  created_at: string | null;
};

type LoginResponse = {
  message: string;
  user: AuthUser;
};

type ForgotPasswordResponse = {
  message: string;
  reset_token?: string;
};

@Injectable({ providedIn: 'root' })
export class AuthService {
  private userSubject = new BehaviorSubject<AuthUser | null>(null);
  user$ = this.userSubject.asObservable();

  private mePollSub?: Subscription;

  constructor(private http: HttpClient, private router: Router) {}

  get user(): AuthUser | null {
    return this.userSubject.value;
  }

  login(username: string, password: string): Observable<LoginResponse> {
    return this.http
      .post<LoginResponse>(
        `${environment.apiBase}/auth/login`,
        { username, password },
        { withCredentials: true }
      )
      .pipe(
        tap((res) => this.userSubject.next(res.user)),
        tap(() => this.startMePolling())
      );
  }

  register(username: string, email: string, password: string): Observable<AuthUser> {
    return this.http
      .post<AuthUser>(
        `${environment.apiBase}/auth/register`,
        { username, email, password },
        { withCredentials: true }
      )
      .pipe(
        tap((user) => this.userSubject.next(user)),
        tap(() => this.startMePolling())
      );
  }

  forgotPassword(email: string): Observable<ForgotPasswordResponse> {
    return this.http.post<ForgotPasswordResponse>(
      `${environment.apiBase}/auth/forgot-password`,
      { email },
      { withCredentials: true }
    );
  }

  resetPassword(token: string, password: string): Observable<{ message: string }> {
    return this.http.post<{ message: string }>(
      `${environment.apiBase}/auth/reset-password`,
      { token, password },
      { withCredentials: true }
    );
  }

  me() {
    return this.http
      .get<AuthUser>(
        `${environment.apiBase}/auth/me`,
        { withCredentials: true }
      )
      .pipe(
        tap((user) => this.userSubject.next(user ?? null)),
        map(() => true),
        catchError(() => {
          this.userSubject.next(null);
          return of(false);
        })
      );
  }

  startMePolling(periodMs = 60_000) {
    if (this.mePollSub) return;

    this.mePollSub = timer(0, periodMs)
      .pipe(switchMap(() => this.me()))
      .subscribe((loggedIn) => {
        if (
          !loggedIn &&
          this.router.url !== '/login' &&
          this.router.url !== '/register' &&
          this.router.url !== '/forgot-password' &&
          !this.router.url.startsWith('/reset-password')
        ) {
          this.router.navigateByUrl('/login');
        }
      });
  }

  stopMePolling() {
    this.mePollSub?.unsubscribe();
    this.mePollSub = undefined;
  }

  logout(): Observable<void> {
    return this.http
      .post<void>(
        `${environment.apiBase}/auth/logout`,
        {},
        { withCredentials: true }
      )
      .pipe(
        tap(() => {
          this.stopMePolling();
          this.userSubject.next(null);
          this.router.navigateByUrl('/login');
        })
      );
  }
}
