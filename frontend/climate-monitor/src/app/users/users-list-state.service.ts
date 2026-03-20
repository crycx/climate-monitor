import { Injectable, inject } from '@angular/core';
import { Router, NavigationStart } from '@angular/router';
import { filter } from 'rxjs/operators';

export interface UsersListState {
  filter: string;
  pageIndex: number;
  pageSize: number;
  sortActive: string;
  sortDirection: 'asc' | 'desc' | '';
}

const DEFAULT_STATE: UsersListState = {
  filter: '',
  pageIndex: 0,
  pageSize: 10,
  sortActive: '',
  sortDirection: '',
};

@Injectable({ providedIn: 'root' })
export class UsersListStateService {
  state: UsersListState = { ...DEFAULT_STATE };

  private router = inject(Router);

  constructor() {
    this.router.events
      .pipe(filter((e): e is NavigationStart => e instanceof NavigationStart))
      .subscribe((e) => {
        // Kui lahkutakse /users “ala” pealt, reset
        if (!e.url.startsWith('/users')) {
          this.reset();
        }
      });
  }

  reset() {
    this.state = { ...DEFAULT_STATE };
  }
}
