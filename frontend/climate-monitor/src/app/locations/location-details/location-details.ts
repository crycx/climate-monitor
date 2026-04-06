import { Component, OnDestroy, OnInit, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { MatSnackBar } from '@angular/material/snack-bar';
import { DialogService } from '../../custom-components/dialog/dialog.service';
import { Snackbar } from '../../custom-components/snackbar/snackbar';
import { ApiService, CreateLocationPayload, UpdateLocationPayload } from '../../services/api';

@Component({
  selector: 'app-location-details',
  standalone: false,
  templateUrl: './location-details.html',
  styleUrl: './location-details.css',
})
export class LocationDetails implements OnInit, OnDestroy {
  loading = signal(false);
  saving = signal(false);
  deleting = signal(false);
  error = signal<string | null>(null);
  success = signal<string | null>(null);

  isCreateMode = false;
  locationId: number | null = null;

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
      description: [''],
    });
  }

  ngOnInit(): void {
    this.route.paramMap.pipe(takeUntil(this.destroy$)).subscribe((params) => {
      const idParam = params.get('id');

      if (idParam === 'add') {
        this.isCreateMode = true;
        this.locationId = null;
        this.form.reset({ name: '', description: '' });
        this.error.set(null);
        return;
      }

      const parsedId = Number(idParam);
      if (!idParam || Number.isNaN(parsedId) || parsedId <= 0) {
        this.error.set('Invalid location id');
        return;
      }

      this.isCreateMode = false;
      this.locationId = parsedId;
      this.loadLocation(parsedId);
    });
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadLocation(id: number): void {
    this.loading.set(true);
    this.error.set(null);

    this.api.getLocation(id).pipe(takeUntil(this.destroy$)).subscribe({
      next: (location) => {
        this.loading.set(false);
        this.form.patchValue({
          name: location.name,
          description: location.description ?? '',
        });
      },
      error: (err) => {
        this.loading.set(false);
        this.error.set(err?.error?.error ?? 'Failed to load location');
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

    if (this.isCreateMode) {
      const payload: CreateLocationPayload = {
        name: value.name,
        description: (value.description || '').trim() || undefined,
      };

      this.saving.set(true);
      this.api.createLocation(payload).pipe(takeUntil(this.destroy$)).subscribe({
        next: (location) => {
          this.saving.set(false);
          this.showSnack('Location created successfully', 'add_location', '#2e7d32');
          this.router.navigate(['/locations', location.id]);
        },
        error: (err) => {
          this.saving.set(false);
          this.error.set(err?.error?.error ?? 'Failed to create location');
        },
      });
      return;
    }

    if (this.locationId === null) {
      this.error.set('Invalid location id');
      return;
    }

    const payload: UpdateLocationPayload = {
      name: value.name,
      description: (value.description || '').trim() || undefined,
    };

    this.saving.set(true);
    this.api.updateLocation(this.locationId, payload).pipe(takeUntil(this.destroy$)).subscribe({
      next: () => {
        this.saving.set(false);
        this.showSnack('Location saved successfully', 'check_circle', '#2e7d32', true);
      },
      error: (err) => {
        this.saving.set(false);
        this.error.set(err?.error?.error ?? 'Failed to save location');
      },
    });
  }

  deleteLocation(): void {
    this.error.set(null);
    this.success.set(null);

    if (this.locationId === null || this.isCreateMode) {
      return;
    }

    const dialogRef = this.dialogService.openPrompt(
      'Delete location',
      'Are you sure you want to delete location:',
      `${this.form.value.name}?`,
      'Devices linked to this location will keep working without location assignment.',
    );

    dialogRef.afterClosed().pipe(takeUntil(this.destroy$)).subscribe((confirmed: boolean) => {
      if (!confirmed) {
        return;
      }

      this.deleting.set(true);
      this.api.deleteLocation(this.locationId!).pipe(takeUntil(this.destroy$)).subscribe({
        next: () => {
          this.deleting.set(false);
          this.showSnack('Location deleted successfully', 'delete_forever', '#b71c1c');
          this.router.navigateByUrl('/locations');
        },
        error: (err) => {
          this.deleting.set(false);
          this.error.set(err?.error?.error ?? 'Failed to delete location');
        },
      });
    });
  }

  back(): void {
    this.router.navigateByUrl('/locations');
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
}
