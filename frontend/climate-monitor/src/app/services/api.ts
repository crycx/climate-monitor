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

export interface ApiReading {
  id: number;
  device_id: number;
  location_id: number | null;
  location_name?: string | null;
  sensor_type: string;
  value: number;
  unit: string;
  timestamp: string | null;
}

export interface ApiLocation {
  id: number;
  user_id: number | null;
  name: string;
  description: string | null;
  created_at: string | null;
  updated_at: string | null;
}

export interface ApiDevice {
  id: number;
  user_id: number;
  location_id: number | null;
  name: string;
  device_type: string | null;
  is_active: boolean;
  last_seen: string | null;
  created_at: string | null;
  api_key?: string;
  can_manage?: boolean;
}

export interface ApiDeviceShare {
  id: number;
  device_id: number;
  shared_with_user_id: number;
  starts_at: string | null;
  ends_at: string | null;
  created_at: string | null;
  shared_with?: {
    id: number;
    username: string;
    email: string;
  } | null;
}

export interface CreateDeviceSharePayload {
  shared_with_user_id?: number;
  shared_with_email: string;
  starts_at: string;
  ends_at: string;
}

export interface DeviceReadingsFilter {
  from_ts?: string;
  to_ts?: string;
  order?: 'asc' | 'desc';
  limit?: number;
}

export interface CreateLocationPayload {
  name: string;
  description?: string;
}

export interface UpdateLocationPayload {
  name?: string;
  description?: string;
}

export interface CreateDevicePayload {
  name: string;
  location_id?: number | null;
  device_type?: string;
  is_active?: boolean;
  api_key?: string;
}

export interface UpdateDevicePayload {
  name?: string;
  location_id?: number | null;
  device_type?: string;
  is_active?: boolean;
  api_key?: string;
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

  listLocations(): Observable<ApiLocation[]> {
    return this.http.get<ApiLocation[]>(`${this.base}/locations`, {
      withCredentials: true,
    });
  }

  createLocation(payload: CreateLocationPayload): Observable<ApiLocation> {
    return this.http.post<ApiLocation>(`${this.base}/locations`, payload, {
      withCredentials: true,
    });
  }

  getLocation(locationId: number): Observable<ApiLocation> {
    return this.http.get<ApiLocation>(`${this.base}/locations/${locationId}`, {
      withCredentials: true,
    });
  }

  updateLocation(locationId: number, payload: UpdateLocationPayload): Observable<ApiLocation> {
    return this.http.patch<ApiLocation>(`${this.base}/locations/${locationId}`, payload, {
      withCredentials: true,
    });
  }

  deleteLocation(locationId: number): Observable<void> {
    return this.http.delete<void>(`${this.base}/locations/${locationId}`, {
      withCredentials: true,
    });
  }

  listDevices(): Observable<ApiDevice[]> {
    return this.http.get<ApiDevice[]>(`${this.base}/devices`, {
      withCredentials: true,
    });
  }

  createDevice(payload: CreateDevicePayload): Observable<ApiDevice> {
    return this.http.post<ApiDevice>(`${this.base}/devices`, payload, {
      withCredentials: true,
    });
  }

  getDevice(deviceId: number): Observable<ApiDevice> {
    return this.http.get<ApiDevice>(`${this.base}/devices/${deviceId}`, {
      withCredentials: true,
    });
  }

  updateDevice(deviceId: number, payload: UpdateDevicePayload): Observable<ApiDevice> {
    return this.http.patch<ApiDevice>(`${this.base}/devices/${deviceId}`, payload, {
      withCredentials: true,
    });
  }

  deleteDevice(deviceId: number): Observable<void> {
    return this.http.delete<void>(`${this.base}/devices/${deviceId}`, {
      withCredentials: true,
    });
  }

  listDeviceShares(deviceId: number): Observable<ApiDeviceShare[]> {
    return this.http.get<ApiDeviceShare[]>(`${this.base}/devices/${deviceId}/shares`, {
      withCredentials: true,
    });
  }

  listShareableUsers(deviceId: number): Observable<ApiUser[]> {
    return this.http.get<ApiUser[]>(`${this.base}/devices/${deviceId}/shareable-users`, {
      withCredentials: true,
    });
  }

  createDeviceShare(deviceId: number, payload: CreateDeviceSharePayload): Observable<ApiDeviceShare> {
    return this.http.post<ApiDeviceShare>(`${this.base}/devices/${deviceId}/shares`, payload, {
      withCredentials: true,
    });
  }

  deleteDeviceShare(deviceId: number, shareId: number): Observable<void> {
    return this.http.delete<void>(`${this.base}/devices/${deviceId}/shares/${shareId}`, {
      withCredentials: true,
    });
  }

  getDeviceReadings(deviceId: number, filter?: DeviceReadingsFilter): Observable<ApiReading[]> {
    const params: Record<string, string | number> = {};
    if (filter?.from_ts) {
      params['from_ts'] = filter.from_ts;
    }
    if (filter?.to_ts) {
      params['to_ts'] = filter.to_ts;
    }
    if (filter?.order) {
      params['order'] = filter.order;
    }
    if (filter?.limit) {
      params['limit'] = filter.limit;
    }

    return this.http.get<ApiReading[]>(`${this.base}/devices/${deviceId}/readings`, {
      withCredentials: true,
      params,
    });
  }

  downloadDeviceReadingsCsv(deviceId: number, filter?: DeviceReadingsFilter): Observable<Blob> {
    const params: Record<string, string | number> = {};
    if (filter?.from_ts) {
      params['from_ts'] = filter.from_ts;
    }
    if (filter?.to_ts) {
      params['to_ts'] = filter.to_ts;
    }

    return this.http.get(`${this.base}/devices/${deviceId}/readings.csv`, {
      withCredentials: true,
      params,
      responseType: 'blob',
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

  getLatestReadings(
    order: 'asc' | 'desc' = 'asc',
    locationId?: number,
    afterId?: number,
    fromTs?: string,
  ): Observable<ApiReading[]> {
    const params: Record<string, string | number> = {
      order,
    };
    if (locationId !== undefined) {
      params['location_id'] = locationId;
    }
    if (afterId !== undefined) {
      params['after_id'] = afterId;
    }
    if (fromTs !== undefined) {
      params['from_ts'] = fromTs;
    }

    return this.http.get<ApiReading[]>(`${this.base}/readings/latest`, {
      withCredentials: true,
      params,
    });
  }
}
