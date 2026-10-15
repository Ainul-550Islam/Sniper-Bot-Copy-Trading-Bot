/**
 * Pure scale helpers for the dependency-free SVG charts (File 3 §7).
 *
 * Everything here is deterministic: no randomness, no time, no DOM. The
 * chart components render these numbers; the unit tests check them directly.
 */

export interface ChartPoint {
  x: string | number;
  y: number;
}

export interface ChartBar {
  label: string;
  value: number;
}

/** Round a range to a "nice" number (1, 2, 5 × 10^n). */
function niceNumber(range: number, round: boolean): number {
  const exponent = Math.floor(Math.log10(range));
  const fraction = range / Math.pow(10, exponent);
  let nice: number;
  if (round) {
    nice = fraction < 1.5 ? 1 : fraction < 3 ? 2 : fraction < 7 ? 5 : 10;
  } else {
    nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  }
  return nice * Math.pow(10, exponent);
}

/**
 * Evenly spaced, human-friendly axis ticks that cover [min, max].
 * Returns [] for non-finite input. A flat series is padded so the axis is
 * never zero-height.
 */
export function niceTicks(min: number, max: number, count = 5): number[] {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return [];
  let lo = Math.min(min, max);
  let hi = Math.max(min, max);
  if (lo === hi) {
    if (lo === 0) {
      lo = -1;
      hi = 1;
    } else {
      const pad = Math.abs(lo) * 0.1;
      lo -= pad;
      hi += pad;
    }
  }
  const steps = Math.max(2, Math.floor(count));
  const range = niceNumber(hi - lo, false);
  const step = niceNumber(range / (steps - 1), true);
  const start = Math.floor(lo / step) * step;
  const end = Math.ceil(hi / step) * step;
  const decimals = Math.max(0, -Math.floor(Math.log10(step)));
  const ticks: number[] = [];
  for (let v = start; v <= end + step / 2; v += step) {
    ticks.push(Number(v.toFixed(decimals)));
  }
  return ticks;
}

/**
 * Map values onto pixel coordinates inside a box of `width` × `height`.
 * `minY`/`maxY` define the vertical domain (y grows upward on screen).
 */
export function linePoints(
  values: number[],
  width: number,
  height: number,
  minY: number,
  maxY: number,
): Array<{ x: number; y: number }> {
  const n = values.length;
  const span = maxY - minY || 1;
  return values.map((value, i) => ({
    x: n <= 1 ? width / 2 : (i / (n - 1)) * width,
    y: height - ((value - minY) / span) * height,
  }));
}

/** SVG `points` attribute string, e.g. "0,10 50,4". */
export function toPointsAttr(points: Array<{ x: number; y: number }>): string {
  return points.map((p) => `${round2(p.x)},${round2(p.y)}`).join(" ");
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

/** True when a value can be plotted (finite number). */
export function isPlottable(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/** Height in pixels of a bar for `value` on a scale that tops out at `max`. */
export function barHeight(value: number, max: number, height: number): number {
  if (!isPlottable(value) || !(max > 0)) return 0;
  return Math.max(0, Math.min(height, (value / max) * height));
}
