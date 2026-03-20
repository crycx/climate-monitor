import { Component, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { Router } from '@angular/router';
import { AuthService } from '../auth/auth.service';

@Component({
  selector: 'app-forgot-password',
  templateUrl: './forgot-password.html',
  standalone: false,
  styleUrls: ['./forgot-password.css'],
})
export class ForgotPassword {
  loading = false;
  error = signal<string | null>(null);
  info = signal<string | null>(null);
  devToken = signal<string | null>(null);
  form: FormGroup;

  constructor(private fb: FormBuilder, private auth: AuthService, private router: Router) {
    this.form = this.fb.group({
      email: ['', [Validators.required, Validators.email]],
    });
  }

  submit() {
    this.error.set(null);
    this.info.set(null);
    this.devToken.set(null);

    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }

    const { email } = this.form.value;
    this.loading = true;

    this.auth.forgotPassword(email!).subscribe({
      next: (res) => {
        this.loading = false;
        this.info.set(res.message);
        if (res.reset_token) {
          this.devToken.set(res.reset_token);
        }
      },
      error: (err) => {
        this.loading = false;
        this.error.set(err?.error?.error ?? 'Request failed');
      },
    });
  }

  openResetPage() {
    const token = this.devToken();
    if (!token) {
      this.router.navigateByUrl('/reset-password');
      return;
    }

    this.router.navigate(['/reset-password'], {
      queryParams: { token },
    });
  }
}
