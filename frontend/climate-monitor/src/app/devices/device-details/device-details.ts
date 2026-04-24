import { Component, OnDestroy, OnInit, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { MatSnackBar } from '@angular/material/snack-bar';
import { DialogService } from '../../custom-components/dialog/dialog.service';
import { Snackbar } from '../../custom-components/snackbar/snackbar';
import {
  ApiUser,
  ApiLocation,
  ApiDeviceShare,
  ApiService,
  CreateDevicePayload,
  CreateDeviceSharePayload,
  UpdateDevicePayload,
} from '../../services/api';

@Component({
  selector: 'app-device-details',
  standalone: false,
  templateUrl: './device-details.html',
  styleUrl: './device-details.css',
})
export class DeviceDetails implements OnInit, OnDestroy {
  loading = signal(false);
  saving = signal(false);
  deleting = signal(false);
  error = signal<string | null>(null);
  success = signal<string | null>(null);
  showGuide = signal(false);
  canManage = signal(true);

  sharesLoading = signal(false);
  shareSaving = signal(false);
  shareError = signal<string | null>(null);
  shares = signal<ApiDeviceShare[]>([]);

  csvExporting = signal(false);
  csvError = signal<string | null>(null);

  isCreateMode = false;
  deviceId: number | null = null;
  deviceOwnerUserId: number | null = null;
  locations = signal<ApiLocation[]>([]);
  shareableUsers = signal<ApiUser[]>([]);

  form: FormGroup;
  shareForm: FormGroup;
  csvForm: FormGroup;
  private destroy$ = new Subject<void>();

  constructor(
    private fb: FormBuilder,
    private route: ActivatedRoute,
    private router: Router,
    private api: ApiService,
    private dialogService: DialogService,
    private snackBar: MatSnackBar,
  ) {
    this.form = this.fb.group({
      name: ['', [Validators.required]],
      device_type: [''],
      location_id: [''],
      is_active: [true],
      api_key: [''],
    });

    this.shareForm = this.fb.group({
      shared_with_user_id: ['', [Validators.required]],
      starts_at: ['', [Validators.required]],
      ends_at: ['', [Validators.required]],
    });

    this.csvForm = this.fb.group({
      from_ts: [''],
      to_ts: [''],
    });
  }

  ngOnInit(): void {
    this.loadLocations();

    this.route.paramMap.pipe(takeUntil(this.destroy$)).subscribe((params) => {
      const idParam = params.get('id');

      if (idParam === 'add') {
        this.isCreateMode = true;
        this.deviceId = null;
        this.canManage.set(true);
        this.shares.set([]);
        this.shareableUsers.set([]);
        this.shareError.set(null);
        this.form.reset({
          name: '',
          device_type: '',
          location_id: '',
          is_active: true,
          api_key: '',
        });
        this.error.set(null);
        return;
      }

      const parsedId = Number(idParam);
      if (!idParam || Number.isNaN(parsedId) || parsedId <= 0) {
        this.error.set('Invalid device id');
        return;
      }

      this.isCreateMode = false;
      this.deviceId = parsedId;
      this.loadDevice(parsedId);
    });
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  private loadLocations(): void {
    this.api.listLocations().pipe(takeUntil(this.destroy$)).subscribe({
      next: (locations) => this.locations.set(locations ?? []),
      error: () => this.locations.set([]),
    });
  }

  private loadDevice(id: number): void {
    this.loading.set(true);
    this.error.set(null);

    this.api.getDevice(id).pipe(takeUntil(this.destroy$)).subscribe({
      next: (device) => {
        this.loading.set(false);
        const canManage = device.can_manage ?? true;
        this.canManage.set(canManage);
        this.deviceOwnerUserId = device.user_id;
        this.form.patchValue({
          name: device.name,
          device_type: device.device_type ?? '',
          location_id: device.location_id ?? '',
          is_active: device.is_active,
          api_key: device.api_key ?? '',
        });

        if (!canManage && !this.isCreateMode) {
          this.form.disable({ emitEvent: false });
        } else {
          this.form.enable({ emitEvent: false });
        }

        if (canManage) {
          this.loadShareableUsers(id);
          this.loadShares(id);
        } else {
          this.shares.set([]);
          this.shareableUsers.set([]);
          this.shareError.set('You can view this device, but sharing is only available for owner/admin.');
        }
      },
      error: (err) => {
        this.loading.set(false);
        this.error.set(err?.error?.error ?? 'Failed to load device');
      },
    });
  }

  save(): void {
    this.error.set(null);
    this.success.set(null);

    if (!this.isCreateMode && !this.canManage()) {
      return;
    }

    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }

    const value = this.form.value;
    const locationId = value.location_id === '' ? null : Number(value.location_id);

    if (this.isCreateMode) {
      const payload: CreateDevicePayload = {
        name: value.name,
        device_type: (value.device_type || '').trim() || undefined,
        location_id: Number.isFinite(locationId) ? locationId : null,
        is_active: !!value.is_active,
        api_key: (value.api_key || '').trim() || undefined,
      };

      this.saving.set(true);
      this.api.createDevice(payload).pipe(takeUntil(this.destroy$)).subscribe({
        next: (device) => {
          this.saving.set(false);
          this.showSnack('Device created successfully', 'devices', '#2e7d32');
          this.router.navigate(['/devices', device.id]);
        },
        error: (err) => {
          this.saving.set(false);
          this.error.set(err?.error?.error ?? 'Failed to create device');
        },
      });
      return;
    }

    if (this.deviceId === null) {
      this.error.set('Invalid device id');
      return;
    }

    const payload: UpdateDevicePayload = {
      name: value.name,
      device_type: (value.device_type || '').trim() || undefined,
      location_id: Number.isFinite(locationId) ? locationId : null,
      is_active: !!value.is_active,
      api_key: (value.api_key || '').trim() || undefined,
    };

    this.saving.set(true);
    this.api.updateDevice(this.deviceId, payload).pipe(takeUntil(this.destroy$)).subscribe({
      next: () => {
        this.saving.set(false);
        this.showSnack('Device saved successfully', 'check_circle', '#2e7d32', true);
      },
      error: (err) => {
        this.saving.set(false);
        this.error.set(err?.error?.error ?? 'Failed to save device');
      },
    });
  }

  deleteDevice(): void {
    this.error.set(null);
    this.success.set(null);

    if (this.deviceId === null || this.isCreateMode || !this.canManage()) {
      return;
    }

    const dialogRef = this.dialogService.openPrompt(
      'Delete device',
      'Are you sure you want to delete device:',
      `${this.form.value.name}?`,
      'This action cannot be undone.',
    );

    dialogRef.afterClosed().pipe(takeUntil(this.destroy$)).subscribe((confirmed: boolean) => {
      if (!confirmed) {
        return;
      }

      this.deleting.set(true);
      this.api.deleteDevice(this.deviceId!).pipe(takeUntil(this.destroy$)).subscribe({
        next: () => {
          this.deleting.set(false);
          this.showSnack('Device deleted successfully', 'delete_forever', '#b71c1c');
          this.router.navigateByUrl('/devices');
        },
        error: (err) => {
          this.deleting.set(false);
          this.error.set(err?.error?.error ?? 'Failed to delete device');
        },
      });
    });
  }

  back(): void {
    this.router.navigateByUrl('/devices');
  }

  exportCsv(): void {
    this.csvError.set(null);

    if (this.deviceId === null || this.isCreateMode) {
      return;
    }

    const fromIso = this.localDateTimeToIso(this.csvForm.value.from_ts);
    const toIso = this.localDateTimeToIso(this.csvForm.value.to_ts);

    if (this.csvForm.value.from_ts && !fromIso) {
      this.csvError.set('Invalid "from" timestamp.');
      return;
    }
    if (this.csvForm.value.to_ts && !toIso) {
      this.csvError.set('Invalid "to" timestamp.');
      return;
    }
    if (fromIso && toIso && fromIso > toIso) {
      this.csvError.set('From timestamp must be earlier than to timestamp.');
      return;
    }

    this.csvExporting.set(true);
    this.api
      .downloadDeviceReadingsCsv(this.deviceId, {
        from_ts: fromIso ?? undefined,
        to_ts: toIso ?? undefined,
      })
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (blob) => {
          this.csvExporting.set(false);
          this.triggerBlobDownload(blob, `device-${this.deviceId}-readings.csv`);
          this.showSnack('CSV export downloaded', 'download_done', '#2e7d32');
        },
        error: (err) => {
          this.csvExporting.set(false);
          this.csvError.set(err?.error?.error ?? 'Failed to export CSV');
        },
      });
  }

  createShare(): void {
    this.shareError.set(null);

    if (this.deviceId === null || this.isCreateMode || !this.canManage()) {
      return;
    }

    if (this.shareForm.invalid) {
      this.shareForm.markAllAsTouched();
      return;
    }

    const startsAtIso = this.localDateTimeToIso(this.shareForm.value.starts_at);
    const endsAtIso = this.localDateTimeToIso(this.shareForm.value.ends_at);
    if (!startsAtIso || !endsAtIso) {
      this.shareError.set('Invalid share period dates.');
      return;
    }
    if (startsAtIso >= endsAtIso) {
      this.shareError.set('Share start must be earlier than share end.');
      return;
    }

    const payload: CreateDeviceSharePayload = {
      shared_with_user_id: Number(this.shareForm.value.shared_with_user_id),
      shared_with_email: '',
      starts_at: startsAtIso,
      ends_at: endsAtIso,
    };

    if (!Number.isInteger(payload.shared_with_user_id) || (payload.shared_with_user_id ?? 0) <= 0) {
      this.shareError.set('Please select a user.');
      return;
    }

    this.shareSaving.set(true);
    this.api.createDeviceShare(this.deviceId, payload).pipe(takeUntil(this.destroy$)).subscribe({
      next: () => {
        this.shareSaving.set(false);
        this.shareForm.reset({
          shared_with_user_id: '',
          starts_at: '',
          ends_at: '',
        });
        this.loadShares(this.deviceId!);
        this.showSnack('Device sharing period created', 'groups', '#2e7d32');
      },
      error: (err) => {
        this.shareSaving.set(false);
        this.shareError.set(err?.error?.error ?? 'Failed to create sharing period');
      },
    });
  }

  deleteShare(share: ApiDeviceShare): void {
    if (this.deviceId === null || this.isCreateMode || !this.canManage()) {
      return;
    }

    const who = share.shared_with?.email || `user #${share.shared_with_user_id}`;
    const dialogRef = this.dialogService.openPrompt(
      'Delete sharing period',
      'Are you sure you want to delete sharing for:',
      who,
      'This revokes access immediately.',
    );

    dialogRef.afterClosed().pipe(takeUntil(this.destroy$)).subscribe((confirmed: boolean) => {
      if (!confirmed) {
        return;
      }

      this.api.deleteDeviceShare(this.deviceId!, share.id).pipe(takeUntil(this.destroy$)).subscribe({
        next: () => {
          this.loadShares(this.deviceId!);
          this.showSnack('Sharing period deleted', 'delete', '#b71c1c');
        },
        error: (err) => {
          this.shareError.set(err?.error?.error ?? 'Failed to delete sharing period');
        },
      });
    });
  }

  sharePeriodLabel(share: ApiDeviceShare): string {
    const startsAt = share.starts_at ? new Date(share.starts_at).toLocaleString('et-EE') : 'N/A';
    const endsAt = share.ends_at ? new Date(share.ends_at).toLocaleString('et-EE') : 'N/A';
    return `${startsAt} - ${endsAt}`;
  }

  private loadShares(deviceId: number): void {
    this.sharesLoading.set(true);
    this.shareError.set(null);

    this.api.listDeviceShares(deviceId).pipe(takeUntil(this.destroy$)).subscribe({
      next: (shares) => {
        this.sharesLoading.set(false);
        this.shares.set(shares ?? []);
      },
      error: (err) => {
        this.sharesLoading.set(false);
        this.shares.set([]);
        this.shareError.set(err?.error?.error ?? 'Failed to load device shares');
      },
    });
  }

  private loadShareableUsers(deviceId: number): void {
    this.api.listShareableUsers(deviceId).pipe(takeUntil(this.destroy$)).subscribe({
      next: (users) => {
        this.shareableUsers.set(users ?? []);
        if ((users ?? []).length === 0) {
          this.shareError.set('No other users available for sharing.');
        }
      },
      error: () => {
        // Fallback for environments where /shareable-users endpoint is not deployed yet.
        this.api.listUsers().pipe(takeUntil(this.destroy$)).subscribe({
          next: (users) => {
            const ownerId = this.deviceOwnerUserId;
            const filtered = (users ?? []).filter((user) => user.id !== ownerId);
            this.shareableUsers.set(filtered);
            if (filtered.length === 0) {
              this.shareError.set('No other users available for sharing. If this is unexpected, backend may need update.');
            }
          },
          error: () => {
            this.shareableUsers.set([]);
            this.shareError.set('Failed to load users for sharing.');
          },
        });
      },
    });
  }

  private localDateTimeToIso(value: string | null | undefined): string | null {
    if (!value) {
      return null;
    }

    const normalized = String(value).trim();
    if (!normalized) {
      return null;
    }

    const etMatch = normalized.match(/^(\d{2})\.(\d{2})\.(\d{4})\s+(\d{2}):(\d{2})$/);
    if (etMatch) {
      const day = Number(etMatch[1]);
      const month = Number(etMatch[2]);
      const year = Number(etMatch[3]);
      const hours = Number(etMatch[4]);
      const minutes = Number(etMatch[5]);

      if (
        Number.isNaN(day) ||
        Number.isNaN(month) ||
        Number.isNaN(year) ||
        Number.isNaN(hours) ||
        Number.isNaN(minutes) ||
        month < 1 ||
        month > 12 ||
        day < 1 ||
        day > 31 ||
        hours < 0 ||
        hours > 23 ||
        minutes < 0 ||
        minutes > 59
      ) {
        return null;
      }

      const date = new Date(year, month - 1, day, hours, minutes, 0, 0);
      if (
        date.getFullYear() !== year ||
        date.getMonth() !== month - 1 ||
        date.getDate() !== day
      ) {
        return null;
      }
      return date.toISOString();
    }

    const date = new Date(normalized);
    if (Number.isNaN(date.getTime())) {
      return null;
    }

    return date.toISOString();
  }

  private triggerBlobDownload(blob: Blob, filename: string): void {
    const url = window.URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    document.body.appendChild(anchor);
    anchor.click();
    document.body.removeChild(anchor);
    window.URL.revokeObjectURL(url);
  }

  private showSnack(text: string, icon: string, iconColor: string, withBack = false): void {
    const snackRef = this.snackBar.openFromComponent(Snackbar, {
      horizontalPosition: 'center',
      verticalPosition: 'bottom',
      panelClass: ['cm-snackbar-dark'],
      data: {
        text,
        icon,
        iconColor,
        durationMs: 1800,
        dismiss: 'OK',
        back: withBack ? 'Back' : null,
      },
    });

    if (withBack) {
      snackRef
        .onAction()
        .pipe(takeUntil(this.destroy$))
        .subscribe(() => this.back());
    }
  }

  generateApiKey(): void {
    if (!this.isCreateMode && !this.canManage()) {
      return;
    }

    const apiKey = this.generateApiKeyValue();
    this.form.patchValue({ api_key: apiKey });
    this.form.get('api_key')?.markAsDirty();
    this.success.set('New API key generated. Save to apply it.');
    this.error.set(null);
  }

  private generateApiKeyValue(): string {
    const bytes = new Uint8Array(32);

    if (typeof window !== 'undefined' && window.crypto?.getRandomValues) {
      window.crypto.getRandomValues(bytes);
    } else {
      for (let i = 0; i < bytes.length; i += 1) {
        bytes[i] = Math.floor(Math.random() * 256);
      }
    }

    return Array.from(bytes)
      .map((byte) => byte.toString(16).padStart(2, '0'))
      .join('');
  }
}
