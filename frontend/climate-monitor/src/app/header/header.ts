import { Component } from '@angular/core';
import { AuthService } from '../auth/auth.service';

@Component({
  selector: 'app-header',
  standalone: false,
  templateUrl: './header.html',
  styleUrls: ['./header.css'],

})
export class Header {
  user$;
  constructor(public auth: AuthService) {
    this.user$ = this.auth.user$;
  }

  logout() {
    this.auth.logout().subscribe();
  }

}
