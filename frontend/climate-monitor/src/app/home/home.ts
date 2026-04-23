import { AfterViewInit, Component, ElementRef, OnDestroy, QueryList, ViewChildren, computed, effect, signal, untracked } from '@angular/core';
import { finalize, Subscription, timeout } from 'rxjs';
import { Chart, type ChartConfiguration, type Plugin, registerables } from 'chart.js';
import { MatSnackBar, MatSnackBarRef } from '@angular/material/snack-bar';
import { CdkDragDrop, moveItemInArray } from '@angular/cdk/drag-drop';
import { ApiDevice, ApiLocation, ApiReading, ApiService } from '../services/api';
import { CondensationRisk, ComfortStatus, EnvironmentMetricsResult, calculateEnvironmentMetrics } from '../services/environment-metrics.util';
import { Snackbar } from '../custom-components/snackbar/snackbar';

Chart.register(...registerables);

const crosshairPlugin: Plugin<'line'> = {
  id: 'crosshair',
  afterDatasetsDraw(chart) {
    const tooltip = chart.tooltip;
    if (!tooltip || !tooltip.getActiveElements().length) {
      return;
    }

    const { ctx, chartArea } = chart;
    const x = tooltip.caretX;

    ctx.save();
    ctx.beginPath();
    ctx.moveTo(x, chartArea.top);
    ctx.lineTo(x, chartArea.bottom);
    ctx.lineWidth = 1;
    ctx.strokeStyle = 'rgba(148, 163, 184, 0.65)';
    ctx.setLineDash([4, 4]);
    ctx.stroke();
    ctx.restore();
  },
};

Chart.register(crosshairPlugin);

type ReadingPoint = {
  ts: string;
  value: number;
};

type TimePoint = {
  x: number;
  y: number;
};

type PeriodTrend = 'Warming' | 'Cooling' | 'Stable' | 'Unknown';

type VariabilityLevel = 'Stable' | 'Moderate' | 'Volatile' | 'Unknown';

type PeriodAnalysis = {
  sampleCount: number;
  trend: PeriodTrend;
  variability: VariabilityLevel;
  tempSwing: number | null;
  humiditySwing: number | null;
  comfortShare: number | null;
  dryShare: number | null;
  humidShare: number | null;
};

type DeviceStats = {
  lastSeenTs: string | null;
  tempLatest: number | null;
  tempMin: number | null;
  tempMax: number | null;
  tempAvg: number | null;
  tempDelta: number | null;
  humLatest: number | null;
  humMin: number | null;
  humMax: number | null;
  humAvg: number | null;
  humDelta: number | null;
  environment: EnvironmentMetricsResult;
  periodAnalysis: PeriodAnalysis;
};

type LocationOption = {
  id: number | null;
  label: string;
};

type RangeOption = {
  hours: number | null;
  label: string;
};

type ComparisonMetric = 'temperature' | 'humidity';

type ComparisonRow = {
  deviceId: number;
  name: string;
  locationName: string | null;
  lastSeenTs: string | null;
  temperature: number | null;
  humidity: number | null;
  tempDelta: number | null;
  humDelta: number | null;
};

@Component({
  selector: 'app-home',
  standalone: false,
  templateUrl: './home.html',
  styleUrl: './home.css',
})
export class Home implements AfterViewInit, OnDestroy {
  @ViewChildren('deviceCanvas')
  deviceCanvases!: QueryList<ElementRef<HTMLCanvasElement>>;

  @ViewChildren('comparisonCanvas')
  comparisonCanvases!: QueryList<ElementRef<HTMLCanvasElement>>;

  loading = signal(true);
  error = signal('');
  lastUpdated = signal('');
  locations = signal<ApiLocation[]>([]);
  devices = signal<ApiDevice[]>([]);
  deviceOrderIds = signal<number[]>([]);
  selectedDeviceIds = signal<number[]>([]);
  compareSelectedDeviceIds = signal<number[]>([]);
  compareSelectedLocationId = signal<number | null>(null);
  compareMode = signal(false);
  viewMode = signal<1 | 2 | 3>(2);
  locationOptions = computed<LocationOption[]>(() => [
    { id: null, label: 'All locations' },
    ...this.locations().map((location) => ({ id: location.id, label: location.name })),
  ]);
  devicesWithReadings = computed(() => {
    const readingDeviceIds = new Set(this.readings().map((r) => r.device_id));
    return this.devices().filter((d) => readingDeviceIds.has(d.id));
  });
  visibleDevices = computed(() => {
    const locationId = this.selectedLocationId();
    const selected = this.selectedDeviceIds();
    const locationDevices =
      locationId === null
        ? this.orderedDevicesWithReadings()
        : this.orderedDevicesWithReadings().filter((d) => d.location_id === locationId);

    if (selected.length === 0) return locationDevices;
    return locationDevices.filter((d) => selected.includes(d.id));
  });
  orderedDevicesWithReadings = computed(() => this.orderDevices(this.devicesWithReadings()));
  compareDeviceOptions = computed(() => {
    const locationId = this.compareSelectedLocationId();
    const devices =
      locationId === null
        ? this.devicesWithReadings()
        : this.devicesWithReadings().filter((d) => d.location_id === locationId);

    return [...devices].sort((a, b) => a.name.localeCompare(b.name, this.appLocale));
  });
  comparisonDevices = computed(() => {
    if (!this.compareMode()) {
      return [] as ApiDevice[];
    }

    const selected = this.compareSelectedDeviceIds();
    const devices = this.compareDeviceOptions();
    const filtered = selected.length === 0 ? devices : devices.filter((d) => selected.includes(d.id));
    return filtered.length >= 2 ? filtered : [];
  });
  compareAvailable = computed(() => this.comparisonDevices().length >= 2);
  comparisonRows = computed<ComparisonRow[]>(() => {
    const statsMap = this.deviceStats();
    return this.comparisonDevices()
      .map((device) => {
        const stats = statsMap.get(device.id);
        return {
          deviceId: device.id,
          name: device.name,
          locationName: this.getLocationName(device.location_id),
          lastSeenTs: stats?.lastSeenTs ?? null,
          temperature: stats?.tempLatest ?? null,
          humidity: stats?.humLatest ?? null,
          tempDelta: stats?.tempDelta ?? null,
          humDelta: stats?.humDelta ?? null,
        };
      })
      .sort((a, b) => a.name.localeCompare(b.name, this.appLocale));
  });
  deviceStats = computed(() => {
    const readings = this.readings();
    const devices = this.devicesWithReadings();
    const map = new Map<number, DeviceStats>();

    const seriesValues = (deviceId: number, sensorType: string): number[] =>
      readings
        .filter((r) => r.device_id === deviceId && r.sensor_type === sensorType && r.timestamp !== null)
        .sort((a, b) => new Date(a.timestamp as string).getTime() - new Date(b.timestamp as string).getTime())
        .map((r) => r.value);

    const stat = (vals: number[]) => {
      if (!vals.length) return { latest: null, min: null, max: null, avg: null, delta: null };
      const latest = vals[vals.length - 1];
      return {
        latest,
        min: Math.min(...vals),
        max: Math.max(...vals),
        avg: vals.reduce((s, v) => s + v, 0) / vals.length,
        delta: vals.length >= 2 ? latest - vals[0] : null,
      };
    };

    const analyzePeriod = (tempVals: number[], humVals: number[]): PeriodAnalysis => {
      const tempStats = stat(tempVals);
      const humStats = stat(humVals);
      const sampleCount = Math.max(tempVals.length, humVals.length);

      const tempSwing =
        tempStats.min === null || tempStats.max === null ? null : tempStats.max - tempStats.min;
      const humiditySwing =
        humStats.min === null || humStats.max === null ? null : humStats.max - humStats.min;

      let trend: PeriodTrend = 'Unknown';
      if (tempStats.delta !== null) {
        if (tempStats.delta > 0.4) {
          trend = 'Warming';
        } else if (tempStats.delta < -0.4) {
          trend = 'Cooling';
        } else {
          trend = 'Stable';
        }
      }

      let variability: VariabilityLevel = 'Unknown';
      if (tempSwing !== null) {
        if (tempSwing <= 1.0) {
          variability = 'Stable';
        } else if (tempSwing <= 3.0) {
          variability = 'Moderate';
        } else {
          variability = 'Volatile';
        }
      }

      if (!humVals.length) {
        return {
          sampleCount,
          trend,
          variability,
          tempSwing,
          humiditySwing,
          comfortShare: null,
          dryShare: null,
          humidShare: null,
        };
      }

      const comfortCount = humVals.filter((value) => value >= 30 && value <= 60).length;
      const dryCount = humVals.filter((value) => value < 30).length;
      const humidCount = humVals.filter((value) => value > 60).length;
      const total = humVals.length;

      return {
        sampleCount,
        trend,
        variability,
        tempSwing,
        humiditySwing,
        comfortShare: (comfortCount / total) * 100,
        dryShare: (dryCount / total) * 100,
        humidShare: (humidCount / total) * 100,
      };
    };

    for (const device of devices) {
      const tempSeries = seriesValues(device.id, 'temperature');
      const humSeries = seriesValues(device.id, 'humidity');
      const t = stat(tempSeries);
      const h = stat(humSeries);
      const lastReading = readings
        .filter((r) => r.device_id === device.id && r.timestamp !== null)
        .reduce<string | null>((best, r) => {
          if (!best) return r.timestamp as string;
          return (r.timestamp as string) > best ? (r.timestamp as string) : best;
        }, null);
      map.set(device.id, {
        lastSeenTs: lastReading,
        tempLatest: t.latest,
        tempMin: t.min,
        tempMax: t.max,
        tempAvg: t.avg,
        tempDelta: t.delta,
        humLatest: h.latest,
        humMin: h.min,
        humMax: h.max,
        humAvg: h.avg,
        humDelta: h.delta,
        environment: calculateEnvironmentMetrics(t.latest, h.latest),
        periodAnalysis: analyzePeriod(tempSeries, humSeries),
      });
    }

    return map;
  });
  rangeOptions: RangeOption[] = [
    { hours: 1, label: 'Last 1 h' },
    { hours: 3, label: 'Last 3 h' },
    { hours: 6, label: 'Last 6 h' },
    { hours: 12, label: 'Last 12 h' },
    { hours: 24, label: 'Last 24 h' },
    { hours: 48, label: 'Last 48 h' },
    { hours: null, label: 'All' },
  ];
  rangeLabel = computed(() =>
    this.rangeOptions.find((o) => o.hours === this.selectedRangeHours())?.label ?? 'All',
  );
  selectedLocationId = signal<number | null>(null);
  selectedRangeHours = signal<number | null>(6);
  refreshRequest = signal<{ nonce: number; incremental: boolean }>({ nonce: 0, incremental: false });
  viewReady = signal(false);
  readings = signal<ApiReading[]>([]);
  isDarkMode = signal(true);
  autoRefreshEnabled = signal(true);
  showDerivedMetrics = signal(true);
  showPeriodInsights = signal(true);

  private locationsSub?: Subscription;
  private devicesSub?: Subscription;
  private deviceCharts = new Map<number, Chart>();
  private comparisonCharts = new Map<ComparisonMetric, Chart>();
  private colorSchemeMedia?: MediaQueryList;
  private colorSchemeListener?: (event: MediaQueryListEvent) => void;
  private readonly autoRefreshMs = 30000;
  private readonly renderDebounceMs = 120;
  private readonly rangeStorageKey = 'home.selectedRangeHours';
  private readonly viewModeStorageKey = 'home.viewMode';
  private readonly compareModeStorageKey = 'home.compareMode';
  private readonly derivedMetricsVisibilityStorageKey = 'home.showDerivedMetrics';
  private readonly periodInsightsVisibilityStorageKey = 'home.showPeriodInsights';
  private readonly deviceOrderStorageKey = 'home.deviceOrderIds';
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
        .getLatestReadings('asc', undefined, afterId, fromTs)
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
              ? this.mergeReadings(this.readings(), readings, fromTs)
              : (readings ?? []);
            this.readings.set(merged);
            const locationSync = this.syncDeviceLocationsFromReadings(merged);
            if (locationSync.shouldReloadLocations) {
              this.loadLocations();
            }
            if (locationSync.devicesChanged) {
              this.loadDevices();
            }
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
            for (const chart of this.deviceCharts.values()) {
              chart.destroy();
            }
            this.deviceCharts.clear();
            this.destroyComparisonCharts();
          },
        });

      onCleanup(() => sub.unsubscribe());
    });

    effect((onCleanup) => {
      if (!this.viewReady()) {
        return;
      }

      const readings = this.readings();
      const visibleDevices = this.visibleDevices();
      const comparisonDevices = this.comparisonDevices();
      this.isDarkMode();
      this.viewMode();

      const debounceId = window.setTimeout(() => {
        this.renderCharts(readings, visibleDevices);
        this.renderComparisonCharts(readings, comparisonDevices);
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

    const storedRange = this.readStoredRangeHours();
    if (storedRange !== undefined) {
      this.selectedRangeHours.set(storedRange);
    }

    const storedViewMode = this.readStoredViewMode();
    if (storedViewMode !== undefined) {
      this.viewMode.set(storedViewMode);
    }

    const storedCompareMode = this.readStoredCompareMode();
    if (storedCompareMode !== undefined) {
      this.compareMode.set(storedCompareMode);
    }

    const storedDerivedMetricsVisibility = this.readStoredDerivedMetricsVisibility();
    if (storedDerivedMetricsVisibility !== undefined) {
      this.showDerivedMetrics.set(storedDerivedMetricsVisibility);
    }

    const storedPeriodInsightsVisibility = this.readStoredPeriodInsightsVisibility();
    if (storedPeriodInsightsVisibility !== undefined) {
      this.showPeriodInsights.set(storedPeriodInsightsVisibility);
    }

    const storedDeviceOrder = this.readStoredDeviceOrderIds();
    if (storedDeviceOrder) {
      this.deviceOrderIds.set(storedDeviceOrder);
    }

    effect(() => {
      const availableIds = this.devicesWithReadings().map((device) => device.id);
      if (availableIds.length === 0) {
        return;
      }
      const normalized = this.normalizeDeviceOrder(availableIds, this.deviceOrderIds());
      if (this.areNumberArraysEqual(normalized, this.deviceOrderIds())) {
        return;
      }
      this.deviceOrderIds.set(normalized);
      this.persistDeviceOrderIds(normalized);
    });
  }

  ngAfterViewInit(): void {
    this.loadLocations();
    this.loadDevices();
    this.viewReady.set(true);
    this.refresh();
  }

  ngOnDestroy(): void {
    this.locationsSub?.unsubscribe();
    this.devicesSub?.unsubscribe();
    for (const chart of this.deviceCharts.values()) {
      chart.destroy();
    }
    this.deviceCharts.clear();
    this.destroyComparisonCharts();
    this.closeLoadingSnack();
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
  }

  onCompareLocationChange(value: number | null): void {
    this.compareSelectedLocationId.set(value);
    const allowedIds = new Set(this.compareDeviceOptions().map((device) => device.id));
    this.compareSelectedDeviceIds.update((ids) => ids.filter((id) => allowedIds.has(id)));
  }

  onRangeChange(value: number | null): void {
    this.selectedRangeHours.set(value);
    this.persistRangeHours(value);
    this.refresh();
  }

  onAutoRefreshChange(checked: boolean): void {
    this.autoRefreshEnabled.set(checked);
  }

  onDerivedMetricsVisibilityChange(checked: boolean): void {
    this.showDerivedMetrics.set(checked);
    this.persistDerivedMetricsVisibility(checked);
  }

  onPeriodInsightsVisibilityChange(checked: boolean): void {
    this.showPeriodInsights.set(checked);
    this.persistPeriodInsightsVisibility(checked);
  }

  onDeviceSelectionChange(ids: number[]): void {
    this.selectedDeviceIds.set(ids);
  }

  onCompareDeviceSelectionChange(ids: number[]): void {
    this.compareSelectedDeviceIds.set(ids);
  }

  onCompareModeChange(checked: boolean): void {
    this.compareMode.set(checked);
    this.persistCompareMode(checked);
  }

  onViewModeChange(mode: 1 | 2 | 3): void {
    for (const chart of this.deviceCharts.values()) {
      chart.destroy();
    }
    this.deviceCharts.clear();
    this.viewMode.set(mode);
    this.persistViewMode(mode);
  }

  onDeviceDrop(event: CdkDragDrop<ApiDevice[]>): void {
    if (event.previousIndex === event.currentIndex) {
      return;
    }

    const visibleDevices = [...event.container.data];
    if (!visibleDevices.length) {
      return;
    }

    moveItemInArray(visibleDevices, event.previousIndex, event.currentIndex);
    const visibleReorderedIds = visibleDevices.map((device) => device.id);
    const visibleSet = new Set(visibleReorderedIds);

    const allOrderedIds = this.orderDevices(this.devicesWithReadings()).map((device) => device.id);
    let pointer = 0;
    const mergedOrder = allOrderedIds.map((id) =>
      visibleSet.has(id) ? visibleReorderedIds[pointer++] : id,
    );

    const normalized = this.normalizeDeviceOrder(allOrderedIds, mergedOrder);
    this.deviceOrderIds.set(normalized);
    this.persistDeviceOrderIds(normalized);
  }

  deltaDirection(value: number | null): 'positive' | 'negative' | 'unchanged' | 'none' {
    if (value === null) return 'none';
    if (value > 0.05) return 'positive';
    if (value < -0.05) return 'negative';
    return 'unchanged';
  }

  deltaIcon(value: number | null): string {
    const dir = this.deltaDirection(value);
    if (dir === 'positive') return 'trending_up';
    if (dir === 'negative') return 'trending_down';
    return 'trending_flat';
  }

  formatDelta(value: number | null, unit: string): string {
    if (value === null) return '';
    const sign = value > 0.05 ? '+' : '';
    return `${sign}${value.toFixed(1)} ${unit}`;
  }

  formatPercent(value: number | null): string {
    if (value === null) {
      return '—';
    }
    return `${value.toFixed(1)}%`;
  }

  periodTrendIcon(trend: PeriodTrend): string {
    if (trend === 'Warming') return 'trending_up';
    if (trend === 'Cooling') return 'trending_down';
    if (trend === 'Stable') return 'trending_flat';
    return 'help_outline';
  }

  shortTrendLabel(trend: PeriodTrend): string {
    if (trend === 'Warming') return 'Warm';
    if (trend === 'Cooling') return 'Cool';
    if (trend === 'Stable') return 'Flat';
    return 'Unknown';
  }

  periodTrendStatusClass(trend: PeriodTrend): 'is-low' | 'is-medium' | 'is-high' | 'is-unknown' {
    if (trend === 'Warming') return 'is-medium';
    if (trend === 'Cooling') return 'is-low';
    if (trend === 'Stable') return 'is-low';
    return 'is-unknown';
  }

  variabilityStatusClass(level: VariabilityLevel): 'is-low' | 'is-medium' | 'is-high' | 'is-unknown' {
    if (level === 'Stable') return 'is-low';
    if (level === 'Moderate') return 'is-medium';
    if (level === 'Volatile') return 'is-high';
    return 'is-unknown';
  }

  shortVariabilityLabel(level: VariabilityLevel): string {
    if (level === 'Stable') return 'Low';
    if (level === 'Moderate') return 'Mid';
    if (level === 'Volatile') return 'High';
    return 'Unknown';
  }

  periodSummaryEt(analysis: PeriodAnalysis): string {
    const trendText =
      analysis.trend === 'Warming'
        ? 'Warming'
        : analysis.trend === 'Cooling'
          ? 'Cooling'
          : analysis.trend === 'Stable'
            ? 'Stable'
            : 'Mixed';

    const comfortText =
      analysis.comfortShare === null
        ? 'comfort unknown'
        : analysis.comfortShare >= 70
          ? 'comfortable period'
          : analysis.comfortShare >= 40
            ? 'partly comfortable'
            : 'uncomfortable period';

    return `${trendText}, ${comfortText}`;
  }

  comfortShareClass(value: number | null): 'is-low' | 'is-medium' | 'is-high' | 'is-unknown' {
    if (value === null) {
      return 'is-unknown';
    }
    if (value >= 70) {
      return 'is-low';
    }
    if (value >= 40) {
      return 'is-medium';
    }
    return 'is-high';
  }

  variabilityClass(level: VariabilityLevel): 'is-stable' | 'is-moderate' | 'is-volatile' | 'is-unknown' {
    if (level === 'Stable') return 'is-stable';
    if (level === 'Moderate') return 'is-moderate';
    if (level === 'Volatile') return 'is-volatile';
    return 'is-unknown';
  }

  comfortStatusClass(status: ComfortStatus): 'is-dry' | 'is-comfortable' | 'is-humid' | 'is-unknown' {
    if (status === 'Too dry') return 'is-dry';
    if (status === 'Comfortable') return 'is-comfortable';
    if (status === 'Too humid') return 'is-humid';
    return 'is-unknown';
  }

  comfortStatusIcon(status: ComfortStatus): string {
    if (status === 'Too dry') return 'air';
    if (status === 'Comfortable') return 'sentiment_satisfied';
    if (status === 'Too humid') return 'water_drop';
    return 'help_outline';
  }

  condensationRiskClass(risk: CondensationRisk): 'is-low' | 'is-medium' | 'is-high' | 'is-unknown' {
    if (risk === 'Low') return 'is-low';
    if (risk === 'Medium') return 'is-medium';
    if (risk === 'High') return 'is-high';
    return 'is-unknown';
  }

  condensationRiskIcon(risk: CondensationRisk): string {
    if (risk === 'Low') return 'check_circle';
    if (risk === 'Medium') return 'warning';
    if (risk === 'High') return 'error';
    return 'help_outline';
  }

  getLocationName(locationId: number | null): string | null {
    if (!locationId) return null;
    return this.locations().find((l) => l.id === locationId)?.name ?? null;
  }

  getLatestValue(deviceId: number, sensorType: 'temperature' | 'humidity'): number | null {
    const latest = this.readings()
      .filter((reading) => reading.device_id === deviceId && reading.sensor_type === sensorType && reading.timestamp)
      .sort((a, b) => new Date(b.timestamp as string).getTime() - new Date(a.timestamp as string).getTime())[0];

    return latest ? latest.value : null;
  }

  private renderCharts(readings: ApiReading[], visibleDevices: ApiDevice[]): void {
    const visibleIds = new Set(visibleDevices.map((d) => d.id));
    for (const [id, chart] of this.deviceCharts) {
      if (!visibleIds.has(id)) {
        chart.destroy();
        this.deviceCharts.delete(id);
      }
    }

    const canvases = this.deviceCanvases.toArray();
    visibleDevices.forEach((device, index) => {
      const canvas = canvases[index]?.nativeElement;
      const deviceReadings = readings.filter((r) => r.device_id === device.id);
      const temp = this.mapSeries(deviceReadings, 'temperature');
      const hum = this.mapSeries(deviceReadings, 'humidity');
      const existing = this.deviceCharts.get(device.id);
      const chart = this.renderLineChart(canvas, existing, temp, hum);
      if (chart) {
        this.deviceCharts.set(device.id, chart);
      } else {
        this.deviceCharts.delete(device.id);
      }
    });
  }

  private renderComparisonCharts(readings: ApiReading[], devices: ApiDevice[]): void {
    if (devices.length < 2) {
      this.destroyComparisonCharts();
      return;
    }

    const canvases = this.comparisonCanvases?.toArray() ?? [];
    this.renderComparisonMetricChart('temperature', canvases[0]?.nativeElement, readings, devices);
    this.renderComparisonMetricChart('humidity', canvases[1]?.nativeElement, readings, devices);
  }

  private renderComparisonMetricChart(
    sensorType: ComparisonMetric,
    canvas: HTMLCanvasElement | undefined,
    readings: ApiReading[],
    devices: ApiDevice[],
  ): void {
    if (!canvas) {
      return;
    }

    const existing = this.comparisonCharts.get(sensorType);
    const dark = this.isDarkMode();
    const axisColor = dark ? '#d0d5dd' : '#344054';
    const gridColor = dark ? 'rgba(208, 213, 221, 0.18)' : 'rgba(52, 64, 84, 0.14)';
    const unit = sensorType === 'temperature' ? '°C' : '%';
    const title = sensorType === 'temperature' ? 'Temperature comparison' : 'Humidity comparison';

    const datasets: ChartConfiguration<'line'>['data']['datasets'] = devices.map((device, index) => {
      const deviceReadings = readings.filter((reading) => reading.device_id === device.id);
      const series = this.toTimeSeries(this.mapSeries(deviceReadings, sensorType));
      const lineColor = this.comparisonSeriesColor(index, 1);

      return {
        label: device.name,
        data: series,
        borderColor: lineColor,
        backgroundColor: this.comparisonSeriesColor(index, 0.18),
        fill: false,
        showLine: true,
        spanGaps: 10 * 60 * 1000,
        tension: 0.25,
        borderWidth: 2,
        pointRadius: 1.5,
      };
    });

    if (existing) {
      existing.data.datasets = datasets;

      const legendLabels = existing.options.plugins?.legend?.labels;
      if (legendLabels) {
        legendLabels.color = axisColor;
      }

      const xScale = existing.options.scales?.['x'];
      const yScale = existing.options.scales?.['y'];

      if (xScale && 'ticks' in xScale && xScale.ticks) {
        xScale.ticks.color = axisColor;
      }
      if (xScale && 'grid' in xScale && xScale.grid) {
        xScale.grid.color = gridColor;
      }
      if (yScale && 'ticks' in yScale && yScale.ticks) {
        yScale.ticks.color = axisColor;
      }
      if (yScale && 'grid' in yScale && yScale.grid) {
        yScale.grid.color = gridColor;
      }
      if (yScale && 'title' in yScale && yScale.title) {
        yScale.title.color = axisColor;
      }

      existing.resize();
      existing.update('none');
      return;
    }

    const config: ChartConfiguration<'line'> = {
      type: 'line',
      data: {
        datasets,
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        interaction: {
          mode: 'nearest',
          axis: 'x',
          intersect: false,
        },
        plugins: {
          legend: {
            position: 'bottom',
            labels: {
              color: axisColor,
            },
          },
          title: {
            display: false,
            text: title,
          },
          tooltip: {
            intersect: false,
            callbacks: {
              title: (items) => {
                const item = items[0];
                const raw = item?.raw as TimePoint | undefined;
                if (!raw) {
                  return '';
                }
                return new Date(raw.x).toLocaleString(this.appLocale);
              },
              label: (context) => {
                const value = typeof context.parsed?.y === 'number' ? context.parsed.y.toFixed(1) : '—';
                return `${context.dataset.label}: ${value} ${unit}`;
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
          y: {
            type: 'linear',
            ticks: {
              color: axisColor,
            },
            title: {
              display: true,
              text: unit,
              color: axisColor,
            },
            grid: {
              color: gridColor,
            },
          },
        },
      },
    };

    this.comparisonCharts.set(sensorType, new Chart(canvas, config));
  }

  private destroyComparisonCharts(): void {
    for (const chart of this.comparisonCharts.values()) {
      chart.destroy();
    }
    this.comparisonCharts.clear();
  }

  private comparisonSeriesColor(index: number, alpha = 1): string {
    const hues = [12, 205, 272, 145, 35, 320, 88, 252];
    const hue = hues[index % hues.length];
    return `hsla(${hue}, 72%, 52%, ${alpha})`;
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

    const nearestValue = (points: TimePoint[], x: number): number | null => {
      if (!points.length) {
        return null;
      }
      let best = points[0];
      let bestDistance = Math.abs(points[0].x - x);
      for (let i = 1; i < points.length; i += 1) {
        const distance = Math.abs(points[i].x - x);
        if (distance < bestDistance) {
          best = points[i];
          bestDistance = distance;
        }
      }
      return best.y;
    };

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

      existing.resize();
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
            spanGaps: 10 * 60 * 1000,
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
            spanGaps: 10 * 60 * 1000,
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
        interaction: {
          mode: 'nearest',
          axis: 'x',
          intersect: false,
        },
        plugins: {
          legend: {
            labels: {
              color: axisColor,
            },
          },
          tooltip: {
            intersect: false,
            callbacks: {
              title: (items) => {
                const item = items[0];
                const raw = item?.raw as TimePoint | undefined;
                if (!raw) {
                  return '';
                }
                return new Date(raw.x).toLocaleString(this.appLocale);
              },
              // footer: (items) => {
              //   const item = items[0];
              //   const raw = item?.raw as TimePoint | undefined;
              //   if (!raw) {
              //     return '';
              //   }

              //   const temp = nearestValue(temperatureData, raw.x);
              //   const hum = nearestValue(humidityData, raw.x);
              //   const tempText = temp === null ? '-' : temp.toFixed(1);
              //   const humText = hum === null ? '-' : hum.toFixed(1);
              //   return `T: ${tempText} °C   H: ${humText} %`;
              // },
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

  private mergeReadings(current: ApiReading[], incoming: ApiReading[], fromTs?: string): ApiReading[] {
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

    let merged = Array.from(byId.values());
    merged.sort((a, b) => {
      const aTs = a.timestamp ? new Date(a.timestamp).getTime() : 0;
      const bTs = b.timestamp ? new Date(b.timestamp).getTime() : 0;
      if (aTs !== bTs) {
        return aTs - bTs;
      }
      return a.id - b.id;
    });

    if (fromTs) {
      const fromMs = new Date(fromTs).getTime();
      merged = merged.filter((r) => !r.timestamp || new Date(r.timestamp).getTime() >= fromMs);
    }

    return merged.slice(-10000);
  }

  private syncDeviceLocationsFromReadings(readings: ApiReading[]): { devicesChanged: boolean; shouldReloadLocations: boolean } {
    const latestByDevice = new Map<number, ApiReading>();
    for (const reading of readings) {
      const existing = latestByDevice.get(reading.device_id);
      if (!existing) {
        latestByDevice.set(reading.device_id, reading);
        continue;
      }

      const existingTs = existing.timestamp ? new Date(existing.timestamp).getTime() : 0;
      const currentTs = reading.timestamp ? new Date(reading.timestamp).getTime() : 0;
      if (currentTs > existingTs || (currentTs === existingTs && reading.id > existing.id)) {
        latestByDevice.set(reading.device_id, reading);
      }
    }

    const currentDevices = this.devices();
    if (!currentDevices.length) {
      return { devicesChanged: false, shouldReloadLocations: false };
    }

    let devicesChanged = false;
    const nextDevices = currentDevices.map((device) => {
      const latest = latestByDevice.get(device.id);
      if (!latest || latest.location_id === device.location_id) {
        return device;
      }
      devicesChanged = true;
      return { ...device, location_id: latest.location_id };
    });

    if (devicesChanged) {
      this.devices.set(nextDevices);
    }

    const locationIds = new Set(this.locations().map((location) => location.id));
    const hasUnknownLocation = nextDevices.some(
      (device) => device.location_id !== null && !locationIds.has(device.location_id),
    );

    return {
      devicesChanged,
      shouldReloadLocations: hasUnknownLocation,
    };
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

  private persistViewMode(mode: 1 | 2 | 3): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.viewModeStorageKey, String(mode));
    } catch {
      // Ignore localStorage write errors.
    }
  }

  private persistCompareMode(enabled: boolean): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.compareModeStorageKey, enabled ? '1' : '0');
    } catch {
      // Ignore localStorage write errors.
    }
  }

  private readStoredViewMode(): 1 | 2 | 3 | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }
    try {
      const raw = window.localStorage.getItem(this.viewModeStorageKey);
      if (raw === '1' || raw === '2' || raw === '3') {
        return Number(raw) as 1 | 2 | 3;
      }
    } catch {
      // ignore
    }
    return undefined;
  }

  private readStoredCompareMode(): boolean | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }
    try {
      const raw = window.localStorage.getItem(this.compareModeStorageKey);
      if (raw === '1') {
        return true;
      }
      if (raw === '0') {
        return false;
      }
    } catch {
      // ignore
    }
    return undefined;
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

  private persistDerivedMetricsVisibility(visible: boolean): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.derivedMetricsVisibilityStorageKey, visible ? '1' : '0');
    } catch {
      // Ignore localStorage write errors.
    }
  }

  private readStoredDerivedMetricsVisibility(): boolean | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }
    try {
      const raw = window.localStorage.getItem(this.derivedMetricsVisibilityStorageKey);
      if (raw === '1') {
        return true;
      }
      if (raw === '0') {
        return false;
      }
    } catch {
      // ignore
    }
    return undefined;
  }

  private persistPeriodInsightsVisibility(visible: boolean): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.periodInsightsVisibilityStorageKey, visible ? '1' : '0');
    } catch {
      // Ignore localStorage write errors.
    }
  }

  private persistDeviceOrderIds(ids: number[]): void {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      window.localStorage.setItem(this.deviceOrderStorageKey, JSON.stringify(ids));
    } catch {
      // Ignore localStorage write errors.
    }
  }

  private readStoredPeriodInsightsVisibility(): boolean | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }
    try {
      const raw = window.localStorage.getItem(this.periodInsightsVisibilityStorageKey);
      if (raw === '1') {
        return true;
      }
      if (raw === '0') {
        return false;
      }
    } catch {
      // ignore
    }
    return undefined;
  }

  private readStoredDeviceOrderIds(): number[] | undefined {
    if (typeof window === 'undefined') {
      return undefined;
    }
    try {
      const raw = window.localStorage.getItem(this.deviceOrderStorageKey);
      if (!raw) {
        return undefined;
      }
      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed)) {
        return undefined;
      }
      return parsed.filter((id): id is number => typeof id === 'number' && Number.isFinite(id));
    } catch {
      return undefined;
    }
  }

  private orderDevices(devices: ApiDevice[]): ApiDevice[] {
    if (devices.length <= 1) {
      return devices;
    }

    const orderIndex = new Map<number, number>();
    this.deviceOrderIds().forEach((id, index) => orderIndex.set(id, index));

    return [...devices].sort((a, b) => {
      const aOrder = orderIndex.get(a.id);
      const bOrder = orderIndex.get(b.id);

      if (aOrder !== undefined && bOrder !== undefined) {
        return aOrder - bOrder;
      }
      if (aOrder !== undefined) {
        return -1;
      }
      if (bOrder !== undefined) {
        return 1;
      }

      return a.name.localeCompare(b.name, this.appLocale);
    });
  }

  private normalizeDeviceOrder(availableIds: number[], preferredOrder: number[]): number[] {
    const availableSet = new Set(availableIds);
    const seen = new Set<number>();
    const normalized: number[] = [];

    for (const id of preferredOrder) {
      if (availableSet.has(id) && !seen.has(id)) {
        normalized.push(id);
        seen.add(id);
      }
    }

    for (const id of availableIds) {
      if (!seen.has(id)) {
        normalized.push(id);
      }
    }

    return normalized;
  }

  private areNumberArraysEqual(a: number[], b: number[]): boolean {
    if (a.length !== b.length) {
      return false;
    }
    for (let i = 0; i < a.length; i += 1) {
      if (a[i] !== b[i]) {
        return false;
      }
    }
    return true;
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

  private loadDevices(): void {
    this.devicesSub?.unsubscribe();
    this.devicesSub = this.api.listDevices().subscribe({
      next: (devices) => this.devices.set(devices ?? []),
      error: () => this.devices.set([]),
    });
  }
}
