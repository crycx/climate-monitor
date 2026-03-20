import { Component, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { Router } from '@angular/router';
import { MatSnackBar } from '@angular/material/snack-bar';
import { AuthService } from '../auth/auth.service';
import { Snackbar } from '../custom-components/snackbar/snackbar';

@Component({
  selector: 'app-change-password',
  templateUrl: './change-password.html',
  styleUrl: './change-password.css',
  standalone: false
})
export class ChangePassword {
  loading = signal(false);
  error = signal<string | null>(null);
  showCurrentPassword = signal(false);
  showNewPassword = signal(false);
  showConfirmPassword = signal(false);

  form: FormGroup;

  constructor(
    private fb: FormBuilder,
    private auth: AuthService,
    private router: Router,
    private snackBar: MatSnackBar,
  ) {
    this.form = this.fb.group({
      currentPassword: ['', [Validators.required]],
      newPassword: ['', [Validators.required]],
      confirmPassword: ['', [Validators.required]],
    });
  }

  submit(): void {
    this.error.set(null);

    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }

    const { currentPassword, newPassword, confirmPassword } = this.form.getRawValue();
    if (newPassword !== confirmPassword) {
      this.error.set('New passwords do not match');
      return;
    }

    this.loading.set(true);
    this.auth.changePassword(currentPassword, newPassword).subscribe({
      next: () => {
        this.loading.set(false);
        this.form.reset();
        const snackRef = this.snackBar.openFromComponent(Snackbar, {
          data: {
            text: 'Password changed successfully',
            icon: 'lock_reset',
            iconColor: '#2e7d32',
            durationMs: 3000,
            dismiss: 'OK',
            back: 'Back',
          },
        });

        snackRef.onAction().subscribe(() => this.router.navigateByUrl('/'));
      },
      error: (err) => {
        this.loading.set(false);
        this.error.set(err?.error?.error ?? 'Failed to change password');
      },
    });
  }

  back(): void {
    this.router.navigateByUrl('/');
  }

  toggleCurrentPasswordVisibility(): void {
    this.showCurrentPassword.update(v => !v);
  }

  toggleNewPasswordVisibility(): void {
    this.showNewPassword.update(v => !v);
  }

  toggleConfirmPasswordVisibility(): void {
    this.showConfirmPassword.update(v => !v);
  }
}
