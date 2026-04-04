import { NgModule } from '@angular/core';
import { RouterModule, Routes } from '@angular/router';
import { AuthGuard } from './auth/auth.guard';
import { Home } from './home/home';
import { Login } from './login/login';
import { Register } from './register/register';
import { ForgotPassword } from './forgot-password/forgot-password';
import { ResetPassword } from './reset-password/reset-password';
import { ChangePassword } from './change-password/change-password';
import { Users } from './users/users';
import { UserDetails } from './users/user-details/user-details';
import { AdminGuard } from './auth/admin.guard';
import { environment } from '../environments/environment';

const registerRoute: Routes = environment.selfRegistrationEnabled
  ? [{ path: 'register', component: Register }]
  : [{ path: 'register', redirectTo: 'login', pathMatch: 'full' }];

const routes: Routes = [
  { path: 'login', component: Login },
  ...registerRoute,
  { path: 'forgot-password', component: ForgotPassword },
  { path: 'reset-password', component: ResetPassword },
  { path: 'change-password', component: ChangePassword, canActivate: [AuthGuard] },
  { path: 'users', component: Users, canActivate: [AuthGuard, AdminGuard] },
  { path: 'users/:id', component: UserDetails, canActivate: [AuthGuard, AdminGuard] },
  { path: '', component: Home, canActivate: [AuthGuard] },
  { path: '**', redirectTo: '' },
];

@NgModule({
  imports: [RouterModule.forRoot(routes)],
  exports: [RouterModule]
})
export class AppRoutingModule { }
