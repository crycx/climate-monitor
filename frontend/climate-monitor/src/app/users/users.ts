import { Component, OnDestroy, OnInit, signal, ViewChild } from '@angular/core';
import { MatTableDataSource } from '@angular/material/table';
import { MatPaginator } from '@angular/material/paginator';
import { MatSort } from '@angular/material/sort';
import { Router } from '@angular/router';
import { UsersListStateService } from './users-list-state.service';

import { ApiService, ApiUser } from '../services/api';
import { FormControl } from '@angular/forms';
import { Subject, takeUntil } from 'rxjs';

export type UserRow = {
  id: number;
  email: string;
  username: string;
  is_admin: boolean;
  created_at: string | null;
};

@Component({
  selector: 'app-users',
  templateUrl: './users.html',
  styleUrls: ['./users.css'],
  standalone: false
})

export class Users implements OnInit, OnDestroy {
  displayedColumns: string[] = ['id', 'username', 'email', 'is_admin', 'created_at'];
  dataSource = new MatTableDataSource<UserRow>([]);
  loading = signal(false);
  filterCtrl = new FormControl<string>('');
  private destroy$ = new Subject<void>();
  private _paginator?: MatPaginator;
  private _sort?: MatSort;
  private restoreScheduled = false;

  private dataReady = false;

  // B-variandi lipud

  private restored = false;

  @ViewChild(MatPaginator)
  set paginator(p: MatPaginator) {
    this._paginator = p;
    this.dataSource.paginator = p;
    this.tryRestore();
  }
  get paginator(): MatPaginator | undefined {
    return this._paginator;
  }

  @ViewChild(MatSort)
  set sort(s: MatSort) {
    this._sort = s;
    this.dataSource.sort = s;
    this.tryRestore();
  }
  get sort(): MatSort | undefined {
    return this._sort;
  }

    constructor(
    private api: ApiService,
    private router: Router,
    public listState: UsersListStateService
  ) {}

  ngOnInit() {
    this.dataSource.filterPredicate = (row, filter) => {
      const f = filter.trim().toLowerCase();
      return `${row.id} ${row.username} ${row.email}`.toLowerCase().includes(f);
    };

    this.filterCtrl.valueChanges.pipe(takeUntil(this.destroy$)).subscribe((val) => {
      const v = (val ?? '').trim().toLowerCase();
      this.dataSource.filter = v;
      if (this.dataSource.paginator) this.dataSource.paginator.firstPage();

      // save state
      this.listState.state.filter = val ?? '';
      this.listState.state.pageIndex = this.paginator?.pageIndex ?? 0;
    });
    this.loading.set(true);
    this.loadUsers();
  }

private tryRestore() {
  if (this.restored) return;
  if (this.restoreScheduled) return;
  if (!this._paginator || !this._sort) return;
  if (!this.dataReady) return;

  const active = this.listState.state.sortActive;
  const dir = (this.listState.state.sortDirection ) as any; // 'asc' | 'desc' | ''

  const pageSize = this.listState.state.pageSize ;
  const pageIndex = this.listState.state.pageIndex;
  const filter = this.listState.state.filter ?? '';

  this.restoreScheduled = true;
  setTimeout(() => {
    if (!this._paginator || !this._sort || this.restored) return;

    // restore filter enne (mõjutab koguarvu)
    this.filterCtrl.setValue(filter, { emitEvent: false });
    this.dataSource.filter = filter.trim().toLowerCase();

    // sort MatSort API kaudu
    this._sort.sort({ id: active, start: dir || 'asc', disableClear: false });
    this._sort.direction = dir;

    // paginator pärast filter/sort restore’i
    this._paginator.pageSize = pageSize;
    const total = this.dataSource.filteredData.length;
    const maxPage = Math.max(0, Math.ceil(total / pageSize) - 1);
    this._paginator.pageIndex = Math.min(pageIndex, maxPage);

    // subscribe AFTER restore
    this._sort.sortChange.pipe(takeUntil(this.destroy$)).subscribe(() => this.saveState());
    this._paginator.page.pipe(takeUntil(this.destroy$)).subscribe(() => this.saveState());

    this.restored = true;
  });
}


  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadUsers() {
    this.api.listUsers().pipe(takeUntil(this.destroy$)).subscribe((rows: ApiUser[]) => {
      const users: UserRow[] = (Array.isArray(rows) ? rows : []).map((user) => ({
        id: user.id,
        username: user.username,
        email: user.email,
        is_admin: user.is_admin,
        created_at: user.created_at,
      }));

      this.loading.set(false);
      this.dataSource.data = users;
      this.dataReady = true;

      this.tryRestore();

      // pärast data load’i: pane paginator tagasi samale lehele (kui võimalik)
    });
  }

  openUser(row: UserRow | 'add') {
    if (row === 'add') {
      this.router.navigate(['/users', 'add']);
    } else {
      this.router.navigate(['/users', row.id]);
      this.saveState();
    }
  }

  private saveState() {
    this.listState.state = {
      filter: this.dataSource.filter ?? '',
      pageIndex: this.paginator?.pageIndex ?? 0,
      pageSize: this.paginator?.pageSize ?? 10,
      sortActive: this.sort?.active ?? 'id',
      sortDirection: this.sort?.direction ?? '',
    };
  }
}
