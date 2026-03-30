import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable } from 'rxjs';
import { environment } from '../../environments/environment';

export interface ApiUser {
  id: number;
  username: string;
  email: string;
  is_admin: boolean;
  created_at: string | null;
}

export interface UpdateUserPayload {
  username?: string;
  email?: string;
  is_admin?: boolean;
  password?: string;
}

export interface CreateUserPayload {
  username: string;
  email: string;
  is_admin?: boolean;
}

export interface CreateUserResponse {
  message: string;
  email_sent: boolean;
  reset_token?: string;
  user: ApiUser;
}

@Injectable({ providedIn: 'root' })
export class ApiService {
  private readonly base = environment.apiBase.replace(/\/$/, '');

  constructor(private http: HttpClient) {}

  listUsers(): Observable<ApiUser[]> {
    return this.http.get<ApiUser[]>(`${this.base}/users`, {
      withCredentials: true,
    });
  }

  createUser(payload: CreateUserPayload): Observable<CreateUserResponse> {
    return this.http.post<CreateUserResponse>(`${this.base}/users`, payload, {
      withCredentials: true,
    });
  }

  getUser(userId: number): Observable<ApiUser> {
    return this.http.get<ApiUser>(`${this.base}/users/${userId}`, {
      withCredentials: true,
    });
  }

  updateUser(userId: number, payload: UpdateUserPayload): Observable<ApiUser> {
    return this.http.patch<ApiUser>(`${this.base}/users/${userId}`, payload, {
      withCredentials: true,
    });
  }

  deleteUser(userId: number): Observable<void> {
    return this.http.delete<void>(`${this.base}/users/${userId}`, {
      withCredentials: true,
    });
  }
}
