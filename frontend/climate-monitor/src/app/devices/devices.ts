import { Component, OnDestroy, OnInit, ViewChild, signal } from '@angular/core';
import { FormControl } from '@angular/forms';
import { MatPaginator } from '@angular/material/paginator';
import { MatSort } from '@angular/material/sort';
import { MatTableDataSource } from '@angular/material/table';
import { Router } from '@angular/router';
import { Subject, takeUntil } from 'rxjs';
import { ApiDevice, ApiLocation, ApiService } from '../services/api';

type DeviceRow = {
  id: number;
  name: string;
  device_type: string | null;
  location_id: number | null;
  is_active: boolean;
  last_seen: string | null;
  created_at: string | null;
};

@Component({
  selector: 'app-devices',
  standalone: false,
  templateUrl: './devices.html',
  styleUrl: './devices.css',
})
export class Devices implements OnInit, OnDestroy {
  displayedColumns: string[] = ['id', 'name', 'device_type', 'location', 'is_active', 'last_seen'];
  dataSource = new MatTableDataSource<DeviceRow>([]);
  loading = signal(false);
  error = signal('');
  filterCtrl = new FormControl<string>('');
  locationsById: Record<number, string> = {};

  private destroy$ = new Subject<void>();

  @ViewChild(MatPaginator) paginator?: MatPaginator;
  @ViewChild(MatSort) sort?: MatSort;

  constructor(private api: ApiService, private router: Router) {}

  ngOnInit(): void {
    this.dataSource.filterPredicate = (row, filter) => {
      const f = filter.trim().toLowerCase();
      const location = this.locationName(row.location_id).toLowerCase();
      return `${row.id} ${row.name} ${row.device_type ?? ''} ${location}`.toLowerCase().includes(f);
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
      next: (locations) => {
        this.locationsById = this.toLocationMap(locations ?? []);
      },
      error: () => {
        this.locationsById = {};
      },
    });

    this.api.listDevices().pipe(takeUntil(this.destroy$)).subscribe({
      next: (rows: ApiDevice[]) => {
        this.dataSource.data = (rows ?? []).map((d) => ({
          id: d.id,
          name: d.name,
          device_type: d.device_type,
          location_id: d.location_id,
          is_active: d.is_active,
          last_seen: d.last_seen,
          created_at: d.created_at,
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
        this.error.set('Failed to load devices.');
        this.loading.set(false);
      },
    });
  }

  openDevice(row: DeviceRow | 'add'): void {
    if (row === 'add') {
      this.router.navigate(['/devices', 'add']);
      return;
    }
    this.router.navigate(['/devices', row.id]);
  }

  locationName(locationId: number | null): string {
    if (locationId === null) {
      return '—';
    }
    return this.locationsById[locationId] ?? `Location #${locationId}`;
  }

  private toLocationMap(locations: ApiLocation[]): Record<number, string> {
    const map: Record<number, string> = {};
    for (const location of locations) {
      map[location.id] = location.name;
    }
    return map;
  }
}
