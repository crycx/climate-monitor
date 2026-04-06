import { Component, OnDestroy, OnInit, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { MatSnackBar } from '@angular/material/snack-bar';
import { ApiService, ApiUser, CreateUserPayload, UpdateUserPayload } from '../../services/api';
import { Snackbar } from '../../custom-components/snackbar/snackbar';
import { DialogService } from '../../custom-components/dialog/dialog.service';

@Component({
  selector: 'app-user-details',
  templateUrl: './user-details.html',
  standalone: false,
  styleUrls: ['./user-details.css']
})
export class UserDetails implements OnInit, OnDestroy {
  loading = signal(false);
  saving = signal(false);
  deleting = signal(false);
  error = signal<string | null>(null);
  success = signal<string | null>(null);
  showPassword = signal(false);

  isCreateMode = false;
  userId: number | null = null;
  createdUserId: number | null = null;
  devResetToken = signal<string | null>(null);

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
      username: ['', [Validators.required]],
      email: ['', [Validators.required, Validators.email]],
      is_admin: [false],
      password: [''],
    });
  }

  ngOnInit(): void {
    this.route.paramMap.pipe(takeUntil(this.destroy$)).subscribe(params => {
      const idParam = params.get('id');

      if (idParam === 'add') {
        this.isCreateMode = true;
        this.userId = null;
        this.form.reset({ username: '', email: '', is_admin: false, password: '' });
        this.error.set(null);
        return;
      }

      const parsedId = Number(idParam);
      if (!idParam || Number.isNaN(parsedId) || parsedId <= 0) {
        this.error.set('Invalid user id');
        return;
      }

      this.isCreateMode = false;
      this.userId = parsedId;
      this.loadUser(parsedId);
    });
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadUser(id: number): void {
    this.loading.set(true);
    this.error.set(null);

    this.api.getUser(id).pipe(takeUntil(this.destroy$)).subscribe({
      next: (user: ApiUser) => {
        this.loading.set(false);
        this.form.patchValue({
          username: user.username,
          email: user.email,
          is_admin: user.is_admin,
          password: '',
        });
      },
      error: (err) => {
        this.loading.set(false);
        this.error.set(err?.error?.error ?? 'Failed to load user');
      }
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
      const createPayload: CreateUserPayload = {
        username: value.username,
        email: value.email,
        is_admin: !!value.is_admin,
      };

      this.saving.set(true);
      this.api.createUser(createPayload).pipe(takeUntil(this.destroy$)).subscribe({
        next: (response) => {
          this.saving.set(false);
          this.success.set(response.message);
          this.createdUserId = response.user.id;
          if (response.reset_token) {
            this.devResetToken.set(response.reset_token);
          } else {
            this.showSnack('User created successfully', 'person_add', '#2e7d32', true);
            this.router.navigate(['/users', response.user.id]);
          }
        },
        error: (err) => {
          this.saving.set(false);
          this.error.set(err?.error?.error ?? 'Failed to create user');
        }
      });
      return;
    }

    if (this.userId === null) {
      this.error.set('Invalid user id');
      return;
    }

    const payload: UpdateUserPayload = {
      username: value.username,
      email: value.email,
      is_admin: !!value.is_admin,
    };

    const password = (value.password ?? '').trim();
    if (password) {
      payload.password = password;
    }

    this.saving.set(true);
    this.api.updateUser(this.userId, payload).pipe(takeUntil(this.destroy$)).subscribe({
      next: (user) => {
        this.saving.set(false);
        this.success.set('User saved');
        this.showSnack('User saved successfully', 'check_circle', '#2e7d32', true);
        this.form.patchValue({
          username: user.username,
          email: user.email,
          is_admin: user.is_admin,
          password: '',
        });
      },
      error: (err) => {
        this.saving.set(false);
        this.error.set(err?.error?.error ?? 'Failed to save user');
      }
    });
  }

  deleteUser(): void {
    this.error.set(null);
    this.success.set(null);

    if (this.userId === null || this.isCreateMode) {
      return;
    }

    const dialogRef = this.dialogService.openPrompt(
      'Delete user',
      'Are you sure you want to delete user:',
      `${this.form.value.username} (${this.form.value.email}) ?`,
      'This action cannot be undone.'
    );

    dialogRef.afterClosed().pipe(takeUntil(this.destroy$)).subscribe((confirmed: boolean) => {
      if (!confirmed) {
        return;
      }

      this.deleting.set(true);
      this.api.deleteUser(this.userId!).pipe(takeUntil(this.destroy$)).subscribe({
        next: () => {
          this.deleting.set(false);
          this.showSnack('User deleted successfully', 'delete_forever', '#b71c1c');
          this.router.navigateByUrl('/users');
        },
        error: (err) => {
          this.deleting.set(false);
          this.error.set(err?.error?.error ?? 'Failed to delete user');
        }
      });
    });
  }

  back(): void {
    this.router.navigateByUrl('/users');
  }

  openResetPage(): void {
    const token = this.devResetToken();
    if (!token) {
      this.router.navigateByUrl('/reset-password');
      return;
    }
    this.router.navigate(['/reset-password'], { queryParams: { token } });
  }

  goToUser(): void {
    if (this.createdUserId !== null) {
      this.router.navigate(['/users', this.createdUserId]);
    }
  }

  togglePasswordVisibility(): void {
    this.showPassword.update(v => !v);
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
