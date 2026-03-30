import { Injectable } from '@angular/core';
import {
  HttpInterceptor,
  HttpRequest,
  HttpHandler,
  HttpEvent,
} from '@angular/common/http';
import { Observable } from 'rxjs';
import { environment } from '../../environments/environment';

@Injectable()
export class CredentialsInterceptor implements HttpInterceptor {
  intercept(req: HttpRequest<any>, next: HttpHandler): Observable<HttpEvent<any>> {
    const apiBase = environment.apiBase ?? '';

    const isApiCall =
      // proxy stiilis: /api/...
      (apiBase.startsWith('/') && req.url.startsWith(apiBase)) ||
      // absolute URL: https://.../api/...
      (!apiBase.startsWith('/') && req.url.startsWith(apiBase)) ||
      // fallback: kui keegi hardcode'ib /api ka ilma env'ita
      req.url.startsWith('/api/');

    if (!isApiCall) return next.handle(req);

    // ära clone'i kui already withCredentials=true
    if (req.withCredentials) return next.handle(req);

    return next.handle(req.clone({ withCredentials: true }));
  }
}
