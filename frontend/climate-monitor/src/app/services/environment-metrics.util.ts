export type ComfortStatus = 'Too dry' | 'Comfortable' | 'Too humid' | 'Unknown';

export type CondensationRisk = 'High' | 'Medium' | 'Low' | 'Unknown';

export interface EnvironmentMetricsResult {
  temperature: number | null;
  humidity: number | null;
  dewPoint: number | null;
  feelsLike: number | null;
  absoluteHumidity: number | null;
  comfortStatus: ComfortStatus;
  condensationRisk: CondensationRisk;
}

const MAGNUS_A = 17.62;
const MAGNUS_B = 243.12;

function sanitizeNumber(value: number | null | undefined): number | null {
  if (typeof value !== 'number' || Number.isNaN(value) || !Number.isFinite(value)) {
    return null;
  }
  return value;
}

function sanitizeHumidity(humidityPercent: number | null | undefined): number | null {
  const humidity = sanitizeNumber(humidityPercent);
  if (humidity === null || humidity < 0 || humidity > 100) {
    return null;
  }
  return humidity;
}

export function calculateDewPointC(
  temperatureC: number | null | undefined,
  humidityPercent: number | null | undefined,
): number | null {
  const t = sanitizeNumber(temperatureC);
  const h = sanitizeHumidity(humidityPercent);
  if (t === null || h === null || h === 0) {
    return null;
  }

  // Magnus formula for dew point in Celsius.
  const gamma = Math.log(h / 100) + (MAGNUS_A * t) / (MAGNUS_B + t);
  return (MAGNUS_B * gamma) / (MAGNUS_A - gamma);
}

function cToF(celsius: number): number {
  return (celsius * 9) / 5 + 32;
}

function fToC(fahrenheit: number): number {
  return ((fahrenheit - 32) * 5) / 9;
}

export function calculateHeatIndexC(
  temperatureC: number | null | undefined,
  humidityPercent: number | null | undefined,
): number | null {
  const t = sanitizeNumber(temperatureC);
  const h = sanitizeHumidity(humidityPercent);
  if (t === null || h === null) {
    return null;
  }

  // Heat index is mainly meaningful for warm, humid conditions.
  if (t < 27 || h < 40) {
    return t;
  }

  const tF = cToF(t);

  // NOAA Rothfusz regression in Fahrenheit.
  let hiF =
    -42.379 +
    2.04901523 * tF +
    10.14333127 * h -
    0.22475541 * tF * h -
    0.00683783 * tF * tF -
    0.05481717 * h * h +
    0.00122874 * tF * tF * h +
    0.00085282 * tF * h * h -
    0.00000199 * tF * tF * h * h;

  // Low-humidity adjustment for very hot air.
  if (h < 13 && tF >= 80 && tF <= 112) {
    hiF -= ((13 - h) / 4) * Math.sqrt((17 - Math.abs(tF - 95)) / 17);
  }

  // High-humidity adjustment for moderate heat.
  if (h > 85 && tF >= 80 && tF <= 87) {
    hiF += ((h - 85) / 10) * ((87 - tF) / 5);
  }

  return fToC(hiF);
}

export function calculateAbsoluteHumidityGm3(
  temperatureC: number | null | undefined,
  humidityPercent: number | null | undefined,
): number | null {
  const t = sanitizeNumber(temperatureC);
  const h = sanitizeHumidity(humidityPercent);
  if (t === null || h === null) {
    return null;
  }

  // Absolute humidity in g/m^3 from saturation vapor pressure and RH.
  const saturationVaporPressure = 6.112 * Math.exp((17.67 * t) / (t + 243.5));
  return (saturationVaporPressure * h * 2.1674) / (273.15 + t);
}

export function calculateComfortStatus(humidityPercent: number | null | undefined): ComfortStatus {
  const h = sanitizeHumidity(humidityPercent);
  if (h === null) {
    return 'Unknown';
  }
  if (h < 30) {
    return 'Too dry';
  }
  if (h <= 60) {
    return 'Comfortable';
  }
  return 'Too humid';
}

export function calculateCondensationRisk(
  temperatureC: number | null | undefined,
  dewPointC: number | null | undefined,
): CondensationRisk {
  const t = sanitizeNumber(temperatureC);
  const dp = sanitizeNumber(dewPointC);
  if (t === null || dp === null) {
    return 'Unknown';
  }

  const spread = t - dp;
  if (spread < 2) {
    return 'High';
  }
  if (spread < 5) {
    return 'Medium';
  }
  return 'Low';
}

export function calculateEnvironmentMetrics(
  temperatureC: number | null | undefined,
  humidityPercent: number | null | undefined,
): EnvironmentMetricsResult {
  const temperature = sanitizeNumber(temperatureC);
  const humidity = sanitizeHumidity(humidityPercent);
  const dewPoint = calculateDewPointC(temperature, humidity);
  const feelsLike = calculateHeatIndexC(temperature, humidity);
  const absoluteHumidity = calculateAbsoluteHumidityGm3(temperature, humidity);

  return {
    temperature,
    humidity,
    dewPoint,
    feelsLike,
    absoluteHumidity,
    comfortStatus: calculateComfortStatus(humidity),
    condensationRisk: calculateCondensationRisk(temperature, dewPoint),
  };
}
