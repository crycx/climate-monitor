import { AfterViewInit, Component, ElementRef, OnDestroy, ViewChild, computed, effect, signal, untracked } from '@angular/core';
import { finalize, Subscription, timeout } from 'rxjs';
import { Chart, type ChartConfiguration, registerables } from 'chart.js';
import { MatSnackBar, MatSnackBarRef } from '@angular/material/snack-bar';
import { ApiLocation, ApiReading, ApiService } from '../services/api';
import { Snackbar } from '../custom-components/snackbar/snackbar';

Chart.register(...registerables);

type ReadingPoint = {
  ts: string;
  value: number;
};

type TimePoint = {
  x: number;
  y: number;
};

type LocationOption = {
  id: number | null;
  label: string;
};

type RangeOption = {
  hours: number | null;
  label: string;
};

@Component({
  selector: 'app-home',
  standalone: false,
  templateUrl: './home.html',
  styleUrl: './home.css',
})
export class Home implements AfterViewInit, OnDestroy {
  @ViewChild('climateCanvas')
  climateCanvas?: ElementRef<HTMLCanvasElement>;

  loading = signal(true);
  error = signal('');
  lastUpdated = signal('');
  locations = signal<ApiLocation[]>([]);
  locationOptions = computed<LocationOption[]>(() => [
    { id: null, label: 'All locations' },
    ...this.locations().map((location) => ({ id: location.id, label: location.name })),
  ]);
  rangeOptions: RangeOption[] = [
    { hours: 1, label: 'Last 1 h' },
    { hours: 3, label: 'Last 3 h' },
    { hours: 6, label: 'Last 6 h' },
    { hours: 12, label: 'Last 12 h' },
    { hours: 24, label: 'Last 24 h' },
    { hours: 48, label: 'Last 48 h' },
    { hours: null, label: 'All' },
  ];
  selectedLocationId = signal<number | null>(null);
  selectedRangeHours = signal<number | null>(6);
  refreshRequest = signal<{ nonce: number; incremental: boolean }>({ nonce: 0, incremental: false });
  viewReady = signal(false);
  readings = signal<ApiReading[]>([]);
  isDarkMode = signal(false);
  autoRefreshEnabled = signal(true);

  private locationsSub?: Subscription;
  private climateChart?: Chart;
  private colorSchemeMedia?: MediaQueryList;
  private colorSchemeListener?: (event: MediaQueryListEvent) => void;
  private readonly autoRefreshMs = 30000;
  private readonly renderDebounceMs = 120;
  private readonly rangeStorageKey = 'home.selectedRangeHours';
  private readonly appLocale = 'et-EE';
  private lastSelectedLocationId: number | null | undefined;
  private loadingSnackRef?: MatSnackBarRef<Snackbar>;

  constructor(
    private api: ApiService,
    private snackBar: MatSnackBar,
  ) {
    effect((onCleanup) => {
      if (!this.viewReady()) {
        return;
      }

      const selectedLocationId = this.selectedLocationId();
      const selectedRangeHours = this.selectedRangeHours();
      const request = this.refreshRequest();
      const currentReadings = untracked(() => this.readings());
      const locationChanged = this.lastSelectedLocationId !== selectedLocationId;
      this.lastSelectedLocationId = selectedLocationId;

      const incremental = request.incremental && !locationChanged;
      const afterId = incremental ? this.maxReadingId(currentReadings) : undefined;
      const fromTs = this.buildFromTs(selectedRangeHours);
      const showLoading = !incremental && currentReadings.length === 0;
      const keepExistingOnError = incremental && currentReadings.length > 0;

      if (showLoading) {
        this.loading.set(true);
        this.openLoadingSnack();
      }
      this.error.set('');

      const sub = this.api
        .getLatestReadings(1000, 'asc', selectedLocationId ?? undefined, afterId, fromTs)
        .pipe(timeout(15000), finalize(() => {
          if (showLoading) {
            this.loading.set(false);
            this.closeLoadingSnack();
          }
        }))
        .subscribe({
          next: (readings) => {
            this.closeLoadingSnack();

            const merged = incremental
              ? this.mergeReadings(this.readings(), readings)
              : (readings ?? []);
            this.readings.set(merged);
            this.lastUpdated.set(this.formatDateTimeEt(new Date()));

            const loadedCount = readings?.length ?? 0;
            if (incremental) {
              this.showChartInfoSnack(`Auto refresh: +${loadedCount} readings (total ${merged.length})`, 'sync');
            } else {
              this.showChartInfoSnack(`Loaded ${loadedCount} readings`, 'insights');
            }
          },
          error: (err: unknown) => {
            if (keepExistingOnError) {
              return;
            }

            this.error.set(
              typeof err === 'object' && err !== null && 'name' in err && (err as { name: string }).name === 'TimeoutError'
                ? 'Loading data timed out. Please try again.'
                : 'Failed to load readings.',
            );
            this.readings.set([]);
            this.climateChart?.destroy();
            this.climateChart = undefined;
          },
        });

      onCleanup(() => sub.unsubscribe());
    });

    effect((onCleanup) => {
      if (!this.viewReady()) {
        return;
      }

      const readings = this.readings();
      this.isDarkMode();

      const debounceId = window.setTimeout(() => {
        this.renderChart(readings);
      }, this.renderDebounceMs);

      onCleanup(() => {
        window.clearTimeout(debounceId);
      });
    });

    effect((onCleanup) => {
      if (!this.viewReady() || !this.autoRefreshEnabled()) {
        return;
      }

      const id = setInterval(() => {
        this.refreshIncremental();
      }, this.autoRefreshMs);

      onCleanup(() => clearInterval(id));
    });

    if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
      this.colorSchemeMedia = window.matchMedia('(prefers-color-scheme: dark)');
      this.isDarkMode.set(this.colorSchemeMedia.matches);
      this.colorSchemeListener = (event: MediaQueryListEvent) => {
        this.isDarkMode.set(event.matches);
      };
      this.colorSchemeMedia.addEventListener('change', this.colorSchemeListener);
    }

    const storedRange = this.readStoredRangeHours();
    if (storedRange !== undefined) {
      this.selectedRangeHours.set(storedRange);
    }
  }

  ngAfterViewInit(): void {
    this.loadLocations();
    this.viewReady.set(true);
    this.refresh();
  }

  ngOnDestroy(): void {
    this.locationsSub?.unsubscribe();
    this.climateChart?.destroy();
    this.closeLoadingSnack();
    if (this.colorSchemeMedia && this.colorSchemeListener) {
      this.colorSchemeMedia.removeEventListener('change', this.colorSchemeListener);
    }
  }

  refresh(): void {
    this.refreshRequest.update((request) => ({
      nonce: request.nonce + 1,
      incremental: false,
    }));
  }

  private refreshIncremental(): void {
    this.refreshRequest.update((request) => ({
      nonce: request.nonce + 1,
      incremental: true,
    }));
  }

  onLocationChange(value: number | null): void {
    this.selectedLocationId.set(value);
    this.refresh();
  }

  onRangeChange(value: number | null): void {
    this.selectedRangeHours.set(value);
    this.persistRangeHours(value);
    this.refresh();
  }

  onAutoRefreshChange(checked: boolean): void {
    this.autoRefreshEnabled.set(checked);
  }

  private renderChart(readings: ApiReading[]): void {
    const temperature = this.mapSeries(readings, 'temperature');
    const humidity = this.mapSeries(readings, 'humidity');

    this.climateChart = this.renderLineChart(
      this.climateCanvas?.nativeElement,
      this.climateChart,
      temperature,
      humidity,
    );
  }

  private mapSeries(readings: ApiReading[], sensorType: string): ReadingPoint[] {
    return readings
      .filter((r) => r.sensor_type === sensorType && r.timestamp !== null)
      .map((r) => ({ ts: r.timestamp as string, value: r.value }))
      .filter((r) => Number.isFinite(r.value));
  }

  private renderLineChart(
    canvas: HTMLCanvasElement | undefined,
    existing: Chart | undefined,
    temperaturePoints: ReadingPoint[],
    humidityPoints: ReadingPoint[],
  ): Chart | undefined {
    if (!canvas) {
      return existing;
    }

    const temperatureData = this.toTimeSeries(temperaturePoints);
    const humidityData = this.toTimeSeries(humidityPoints);
    const dark = this.isDarkMode();
    const axisColor = dark ? '#d0d5dd' : '#344054';
    const gridColor = dark ? 'rgba(208, 213, 221, 0.18)' : 'rgba(52, 64, 84, 0.14)';

    if (existing) {
      const tempDataset = existing.data.datasets[0];
      const humDataset = existing.data.datasets[1];

      if (tempDataset) {
        tempDataset.data = temperatureData;
      }
      if (humDataset) {
        humDataset.data = humidityData;
      }

      const legendLabels = existing.options.plugins?.legend?.labels;
      if (legendLabels) {
        legendLabels.color = axisColor;
      }

      const xScale = existing.options.scales?.['x'];
      const yTempScale = existing.options.scales?.['yTemp'];
      const yHumScale = existing.options.scales?.['yHum'];

      if (xScale && 'ticks' in xScale && xScale.ticks) {
        xScale.ticks.color = axisColor;
      }
      if (xScale && 'grid' in xScale && xScale.grid) {
        xScale.grid.color = gridColor;
      }

      if (yTempScale && 'ticks' in yTempScale && yTempScale.ticks) {
        yTempScale.ticks.color = axisColor;
      }
      if (yTempScale && 'grid' in yTempScale && yTempScale.grid) {
        yTempScale.grid.color = gridColor;
      }
      if (yTempScale && 'title' in yTempScale && yTempScale.title) {
        yTempScale.title.color = axisColor;
      }

      if (yHumScale && 'ticks' in yHumScale && yHumScale.ticks) {
        yHumScale.ticks.color = axisColor;
      }
      if (yHumScale && 'grid' in yHumScale && yHumScale.grid) {
        yHumScale.grid.color = gridColor;
      }
      if (yHumScale && 'title' in yHumScale && yHumScale.title) {
        yHumScale.title.color = axisColor;
      }

      existing.update('none');
      return existing;
    }

    const config: ChartConfiguration<'line'> = {
      type: 'line',
      data: {
        datasets: [
          {
            label: 'Temperature (°C)',
            data: temperatureData,
            yAxisID: 'yTemp',
            borderColor: '#e25523',
            backgroundColor: '#e2552333',
            fill: false,
            showLine: true,
            spanGaps: true,
            tension: 0.25,
            borderWidth: 2,
            pointRadius: 2,
          },
          {
            label: 'Humidity (%)',
            data: humidityData,
            yAxisID: 'yHum',
            borderColor: '#1f8fdd',
            backgroundColor: '#1f8fdd33',
            fill: false,
            showLine: true,
            spanGaps: true,
            tension: 0.25,
            borderWidth: 2,
            pointRadius: 2,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        plugins: {
          legend: {
            labels: {
              color: axisColor,
            },
          },
          tooltip: {
            callbacks: {
              title: (items) => {
                const item = items[0];
                const raw = item?.raw as TimePoint | undefined;
                if (!raw) {
                  return '';
                }
                return new Date(raw.x).toLocaleString(this.appLocale);
              },
            },
          },
        },
        scales: {
          x: {
            type: 'linear',
            ticks: {
              stepSize: 5 * 60 * 1000,
              color: axisColor,
              callback: (value) =>
                new Date(Number(value)).toLocaleTimeString(this.appLocale, {
                  hour: '2-digit',
                  minute: '2-digit',
                }),
            },
            grid: {
              color: gridColor,
            },
          },
          yTemp: {
            type: 'linear',
            position: 'left',
            ticks: {
              color: axisColor,
            },
            title: {
              display: true,
              text: '°C',
              color: axisColor,
            },
            grid: {
              color: gridColor,
            },
          },
          yHum: {
            type: 'linear',
            position: 'right',
            ticks: {
              color: axisColor,
            },
            title: {
              display: true,
              text: '%',
              color: axisColor,
            },
            grid: {
              drawOnChartArea: false,
              color: gridColor,
            },
          },
        },
      },
    };

    return new Chart(canvas, config);
  }

  private toTimeSeries(points: ReadingPoint[]): TimePoint[] {
    return points
      .map((p) => ({ x: new Date(p.ts).getTime(), y: p.value }))
      .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y))
      .sort((a, b) => a.x - b.x);
  }

  private maxReadingId(readings: ApiReading[]): number | undefined {
    if (!readings.length) {
      return undefined;
    }

    let max = readings[0].id;
    for (const reading of readings) {
      if (reading.id > max) {
        max = reading.id;
      }
    }
    return max;
  }

  private mergeReadings(current: ApiReading[], incoming: ApiReading[]): ApiReading[] {
    if (!incoming.length) {
      return current;
    }

    const byId = new Map<number, ApiReading>();
    for (const reading of current) {
      byId.set(reading.id, reading);
    }
    for (const reading of incoming) {
      byId.set(reading.id, reading);
    }

    const merged = Array.from(byId.values());
    merged.sort((a, b) => {
      const aTs = a.timestamp ? new Date(a.timestamp).getTime() : 0;
      const bTs = b.timestamp ? new Date(b.timestamp).getTime() : 0;
      if (aTs !== bTs) {
        return aTs - bTs;
      }
      return a.id - b.id;
    });

    return merged.slice(-1000);
  }

  private buildFromTs(hours: number | null): string | undefined {
    if (hours === null) {
      return undefined;
    }
    const from = new Date(Date.now() - hours * 60 * 60 * 1000);
    return from.toISOString();
  }

  private formatDateTimeEt(date: Date): string {
    const pad = (value: number) => value.toString().padStart(2, '0');
    const day = pad(date.getDate());
    const month = pad(date.getMonth() + 1);
    const year = date.getFullYear();
    const hours = pad(date.getHours());
    const minutes = pad(date.getMinutes());
    const seconds = pad(date.getSeconds());
    return `${day}.${month}.${year}, ${hours}:${minutes}:${seconds}`;
  }

  private persistRangeHours(hours: number | null): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.rangeStorageKey, JSON.stringify(hours));
    } catch {
      // Ignore localStorage write errors (private mode/quota/security restrictions).
    }
  }

  private readStoredRangeHours(): number | null | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }

    try {
      const raw = window.localStorage.getItem(this.rangeStorageKey);
      if (raw === null) {
        return undefined;
      }

      const parsed = JSON.parse(raw) as number | null;
      const isValid = this.rangeOptions.some((option) => option.hours === parsed);
      return isValid ? parsed : undefined;
    } catch {
      return undefined;
    }
  }

  private openLoadingSnack(): void {
    if (this.loadingSnackRef) {
      return;
    }

    this.loadingSnackRef = this.snackBar.openFromComponent(Snackbar, {
      horizontalPosition: 'center',
      verticalPosition: 'bottom',
      panelClass: ['cm-snackbar-dark'],
      data: {
        text: 'Loading data',
        icon: 'hourglass_top',
        iconColor: '#38bdf8',
        dots: true,
        durationMs: 3000,
      },
    });

    this.loadingSnackRef.afterDismissed().subscribe(() => {
      this.loadingSnackRef = undefined;
    });
  }

  private closeLoadingSnack(): void {
    if (!this.loadingSnackRef) {
      return;
    }

    this.loadingSnackRef.dismiss();
    this.loadingSnackRef = undefined;
  }

  private showChartInfoSnack(text: string, icon = 'info'): void {
    this.snackBar.openFromComponent(Snackbar, {
      horizontalPosition: 'center',
      verticalPosition: 'bottom',
      panelClass: ['cm-snackbar-dark'],
      data: {
        text,
        icon,
        iconColor: '#38bdf8',
        durationMs: 3000,
        dismiss: 'OK',
      },
    });
  }

  private loadLocations(): void {
    this.locationsSub?.unsubscribe();
    this.locationsSub = this.api.listLocations().subscribe({
      next: (locations) => {
        this.locations.set(locations ?? []);
        const selected = this.selectedLocationId();
        if (selected !== null && !(locations ?? []).some((location) => location.id === selected)) {
          this.selectedLocationId.set(null);
        }
      },
      error: () => {
        this.locations.set([]);
      },
    });
  }
}
