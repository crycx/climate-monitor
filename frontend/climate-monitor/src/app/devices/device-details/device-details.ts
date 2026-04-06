import { Component, OnDestroy, OnInit, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { MatSnackBar } from '@angular/material/snack-bar';
import { DialogService } from '../../custom-components/dialog/dialog.service';
import { Snackbar } from '../../custom-components/snackbar/snackbar';
import {
  ApiLocation,
  ApiService,
  CreateDevicePayload,
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

  isCreateMode = false;
  deviceId: number | null = null;
  locations = signal<ApiLocation[]>([]);

  form: FormGroup;
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
  }

  ngOnInit(): void {
    this.loadLocations();

    this.route.paramMap.pipe(takeUntil(this.destroy$)).subscribe((params) => {
      const idParam = params.get('id');

      if (idParam === 'add') {
        this.isCreateMode = true;
        this.deviceId = null;
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
        this.form.patchValue({
          name: device.name,
          device_type: device.device_type ?? '',
          location_id: device.location_id ?? '',
          is_active: device.is_active,
          api_key: device.api_key ?? '',
        });
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

    if (this.deviceId === null || this.isCreateMode) {
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
