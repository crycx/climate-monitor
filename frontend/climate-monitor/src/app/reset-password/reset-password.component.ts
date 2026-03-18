import { Component, signal } from '@angular/core';
import { FormBuilder, FormGroup, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { AuthService } from '../auth/auth.service';

@Component({
  selector: 'app-reset-password',
  templateUrl: './reset-password.component.html',
  standalone: false,
  styleUrls: ['./reset-password.component.css'],
})
export class ResetPasswordComponent {
  loading = false;
  error = signal<string | null>(null);
  info = signal<string | null>(null);
  form: FormGroup;

  constructor(
    private fb: FormBuilder,
    private auth: AuthService,
    private route: ActivatedRoute,
    private router: Router
  ) {
    this.form = this.fb.group({
      token: ['', [Validators.required]],
      password: ['', [Validators.required, Validators.minLength(6)]],
      confirmPassword: ['', [Validators.required]],
    });

    const token = this.route.snapshot.queryParamMap.get('token');
    if (token) {
      this.form.patchValue({ token });
    }
  }

  submit() {
    this.error.set(null);
    this.info.set(null);

    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }

    const { token, password, confirmPassword } = this.form.value;
    if (password !== confirmPassword) {
      this.error.set('Passwords do not match');
      return;
    }

    this.loading = true;

    this.auth.resetPassword(token!, password!).subscribe({
      next: (res) => {
        this.loading = false;
        this.info.set(res.message);
        setTimeout(() => this.router.navigateByUrl('/login'), 1000);
      },
      error: (err) => {
        this.loading = false;
        this.error.set(err?.error?.error ?? 'Reset failed');
      },
    });
  }
}
