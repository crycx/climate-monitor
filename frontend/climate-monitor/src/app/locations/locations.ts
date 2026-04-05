import { Component, OnDestroy, OnInit, ViewChild, signal } from '@angular/core';
import { FormControl } from '@angular/forms';
import { MatPaginator } from '@angular/material/paginator';
import { MatSort } from '@angular/material/sort';
import { MatTableDataSource } from '@angular/material/table';
import { Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { ApiLocation, ApiService } from '../services/api';

type LocationRow = {
  id: number;
  name: string;
  description: string | null;
  created_at: string | null;
};

@Component({
  selector: 'app-locations',
  standalone: false,
  templateUrl: './locations.html',
  styleUrl: './locations.css',
})
export class Locations implements OnInit, OnDestroy {
  displayedColumns: string[] = ['id', 'name', 'description', 'created_at'];
  dataSource = new MatTableDataSource<LocationRow>([]);
  loading = signal(false);
  error = signal('');
  filterCtrl = new FormControl<string>('');

  private destroy$ = new Subject<void>();

  @ViewChild(MatPaginator) paginator?: MatPaginator;
  @ViewChild(MatSort) sort?: MatSort;

  constructor(private api: ApiService, private router: Router) {}

  ngOnInit(): void {
    this.dataSource.filterPredicate = (row, filter) => {
      const f = filter.trim().toLowerCase();
      return `${row.id} ${row.name} ${row.description ?? ''}`.toLowerCase().includes(f);
    };

    this.filterCtrl.valueChanges.pipe(takeUntil(this.destroy$)).subscribe((value) => {
      this.dataSource.filter = (value ?? '').trim().toLowerCase();
      this.paginator?.firstPage();
    });

    this.refresh();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  refresh(): void {
    this.loading.set(true);
    this.error.set('');
    this.api.listLocations().pipe(takeUntil(this.destroy$)).subscribe({
      next: (rows: ApiLocation[]) => {
        this.dataSource.data = (rows ?? []).map((r) => ({
          id: r.id,
          name: r.name,
          description: r.description,
          created_at: r.created_at,
        }));

        if (this.sort) {
          this.dataSource.sort = this.sort;
        }
        if (this.paginator) {
          this.dataSource.paginator = this.paginator;
        }

        this.loading.set(false);
      },
      error: () => {
        this.error.set('Failed to load locations.');
        this.loading.set(false);
      },
    });
  }

  openLocation(row: LocationRow | 'add'): void {
    if (row === 'add') {
      this.router.navigate(['/locations', 'add']);
      return;
    }
    this.router.navigate(['/locations', row.id]);
  }
}
