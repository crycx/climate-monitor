import { LOCALE_ID, NgModule, provideBrowserGlobalErrorListeners } from '@angular/core';
import { BrowserModule } from '@angular/platform-browser';
import { registerLocaleData } from '@angular/common';
import localeEt from '@angular/common/locales/et';

import { BrowserAnimationsModule } from '@angular/platform-browser/animations';
import { HttpClientModule, HTTP_INTERCEPTORS } from '@angular/common/http';
import { ReactiveFormsModule, FormsModule } from '@angular/forms';
import { RouterModule } from '@angular/router';
import { MatDialogModule } from '@angular/material/dialog';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatSnackBarModule } from '@angular/material/snack-bar';
import { MatCardModule } from '@angular/material/card';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatToolbarModule } from '@angular/material/toolbar';
import { MatMenuModule } from '@angular/material/menu';
import { MatTableModule } from '@angular/material/table';
import { MatPaginatorModule } from '@angular/material/paginator';
import { MatSortModule } from '@angular/material/sort';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTooltipModule } from '@angular/material/tooltip';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatSelectModule } from '@angular/material/select';
import { ServiceWorkerModule } from '@angular/service-worker';
import { Snackbar } from './custom-components/snackbar/snackbar';
import { CredentialsInterceptor } from './auth/credentials.interceptor';
import { environment } from '../environments/environment';

import { AppRoutingModule } from './app-routing-module';
import { App } from './app';
import { PromptDialog } from './custom-components/dialog/dialog.service';
import { Home } from './home/home';
import { Login } from './login/login';
import { Header } from './header/header';
import { Register } from './register/register';
import { ForgotPassword } from './forgot-password/forgot-password';
import { ResetPassword } from './reset-password/reset-password';
import { ChangePassword } from './change-password/change-password';
import { Devices } from './devices/devices';
import { DeviceDetails } from './devices/device-details/device-details';
import { Locations } from './locations/locations';
import { LocationDetails } from './locations/location-details/location-details';
import { Users } from './users/users';
import { UserDetails } from './users/user-details/user-details';

registerLocaleData(localeEt);


@NgModule({
  declarations: [
    App,
    Snackbar,
    PromptDialog,
    Home,
    Login,
    Header,
    Register,
    ForgotPassword,
    ResetPassword,
    ChangePassword,
    Devices,
    DeviceDetails,
    Locations,
    LocationDetails,
    Users,
    UserDetails,
  ],
  imports: [
    BrowserModule,
    BrowserAnimationsModule,
    RouterModule,
    HttpClientModule,
    ReactiveFormsModule,
    FormsModule,
    MatDialogModule,
    MatButtonModule,
    MatIconModule,
    MatSnackBarModule,
    MatCardModule,
    MatFormFieldModule,
    MatInputModule,
    MatToolbarModule,
    MatMenuModule,
    MatTableModule,
    MatPaginatorModule,
    MatSortModule,
    MatProgressBarModule,
    MatTooltipModule,
    MatCheckboxModule,
    MatSelectModule,
    ServiceWorkerModule.register('ngsw-worker.js', {
      enabled: environment.production,
      registrationStrategy: 'registerWhenStable:30000',
    }),
    AppRoutingModule,
  ],
  providers: [
    provideBrowserGlobalErrorListeners(),
    { provide: HTTP_INTERCEPTORS, useClass: CredentialsInterceptor, multi: true },
    { provide: LOCALE_ID, useValue: 'et-EE' },
  ],
  bootstrap: [App],
})
export class AppModule {}
