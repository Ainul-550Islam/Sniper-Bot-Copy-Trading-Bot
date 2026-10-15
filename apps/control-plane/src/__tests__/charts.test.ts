/**
 * Chart scale helpers (File 3 §7). Pure functions only: the SVG components
 * are thin renderers over these, so the numbers are what must stay correct.
 */
import { describe, expect, it } from "vitest";

import { barHeight, isPlottable, linePoints, niceTicks, toPointsAttr } from "../components/charts/scale";

describe("niceTicks", () => {
  it("covers the data range with round steps", () => {
    const ticks = niceTicks(3, 97, 5);
    const first = ticks[0] ?? Number.NaN;
    const second = ticks[1] ?? Number.NaN;
    const last = ticks[ticks.length - 1] ?? Number.NaN;
    expect(first).toBeLessThanOrEqual(3);
    expect(last).toBeGreaterThanOrEqual(97);
    const step = second - first;
    for (let i = 1; i < ticks.length; i++) {
      expect((ticks[i] ?? Number.NaN) - (ticks[i - 1] ?? Number.NaN)).toBeCloseTo(step, 9);
    }
  });

  it("returns [] for non-finite input", () => {
    expect(niceTicks(Number.NaN, 1)).toEqual([]);
    expect(niceTicks(0, Number.POSITIVE_INFINITY)).toEqual([]);
  });

  it("pads a flat series so the axis is never zero-height", () => {
    const ticks = niceTicks(5, 5, 5);
    expect(ticks.length).toBeGreaterThan(1);
    expect(ticks[ticks.length - 1] ?? 0).toBeGreaterThan(ticks[0] ?? 0);
  });

  it("handles a flat zero series", () => {
    const ticks = niceTicks(0, 0, 5);
    expect(ticks[0] ?? 0).toBeLessThan(0);
    expect(ticks[ticks.length - 1] ?? 0).toBeGreaterThan(0);
  });

  it("handles negative PnL ranges", () => {
    const ticks = niceTicks(-250, 80, 5);
    expect(ticks[0] ?? 0).toBeLessThanOrEqual(-250);
    expect(ticks[ticks.length - 1] ?? 0).toBeGreaterThanOrEqual(80);
  });
});

describe("linePoints", () => {
  it("maps the minimum to the bottom and the maximum to the top", () => {
    const pts = linePoints([0, 10], 100, 50, 0, 10);
    expect(pts[0]).toEqual({ x: 0, y: 50 });
    expect(pts[1]).toEqual({ x: 100, y: 0 });
  });

  it("centres a single point horizontally", () => {
    expect(linePoints([4], 100, 50, 0, 10)[0]?.x).toBe(50);
  });

  it("does not divide by zero on a flat domain", () => {
    const pts = linePoints([3, 3], 100, 50, 3, 3);
    expect(Number.isFinite(pts[0]?.y ?? Number.NaN)).toBe(true);
  });

  it("serialises points for an SVG polyline", () => {
    expect(toPointsAttr([{ x: 0, y: 10.004 }, { x: 50.5, y: 3 }])).toBe("0,10 50.5,3");
  });
});

describe("barHeight", () => {
  it("scales linearly and clamps to the box", () => {
    expect(barHeight(50, 100, 200)).toBe(100);
    expect(barHeight(300, 100, 200)).toBe(200);
    expect(barHeight(-5, 100, 200)).toBe(0);
  });

  it("returns 0 for a missing or zero maximum", () => {
    expect(barHeight(5, 0, 200)).toBe(0);
    expect(barHeight(Number.NaN, 100, 200)).toBe(0);
  });
});

describe("isPlottable", () => {
  it("rejects NaN, infinities and non-numbers", () => {
    expect(isPlottable(1.5)).toBe(true);
    expect(isPlottable(Number.NaN)).toBe(false);
    expect(isPlottable(Number.POSITIVE_INFINITY)).toBe(false);
    expect(isPlottable("3")).toBe(false);
    expect(isPlottable(null)).toBe(false);
  });
});
