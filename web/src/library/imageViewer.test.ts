import { describe, expect, it } from "vitest";

import { actualSizeState, clampPan, clampScale, fitScale, fitState, panBy, scaleBounds, wheelFactor, zoomAt } from "./imageViewer";

const image = { w: 1000, h: 2000 };
const view = { w: 800, h: 600 };

describe("fit and 100%", () => {
  it("fits the whole image and centres it", () => {
    expect(fitScale(image, view)).toBeCloseTo(0.3);
    const s = fitState(image, view);
    expect(s.scale).toBeCloseTo(0.3);
    expect(s.x).toBeCloseTo((800 - 300) / 2);
    expect(s.y).toBeCloseTo(0);
  });
  it("shows 100% as one image pixel per screen pixel", () => {
    const s = actualSizeState(image, view);
    expect(s.scale).toBe(1);
    expect(s.x).toBe(0); // 1000 wide is wider than 800: pinned to the left edge
    expect(s.y).toBe(0);
  });
  it("centres a small image at 100%", () => {
    const s = actualSizeState({ w: 200, h: 100 }, view);
    expect(s).toEqual({ scale: 1, x: 300, y: 250 });
  });
  it("tolerates a zero-sized image or viewport", () => {
    expect(fitScale({ w: 0, h: 0 }, view)).toBe(1);
  });
});

describe("zoom", () => {
  it("clamps to the bounds", () => {
    const { min, max } = scaleBounds(image, view);
    expect(clampScale(0.0001, image, view)).toBeCloseTo(min);
    expect(clampScale(1000, image, view)).toBe(max);
    expect(clampScale(Number.NaN, image, view)).toBeCloseTo(0.3);
  });
  it("keeps the point under the cursor fixed", () => {
    const start = { scale: 1, x: -100, y: -100 };
    const next = zoomAt(start, 2, 400, 300, image, view);
    // The image point under (400, 300) before: ((400+100)/1, (300+100)/1)
    expect((400 - next.x) / next.scale).toBeCloseTo(500);
    expect((300 - next.y) / next.scale).toBeCloseTo(400);
  });
  it("cannot zoom past the maximum", () => {
    let s = fitState(image, view);
    for (let i = 0; i < 50; i++) s = zoomAt(s, 2, 400, 300, image, view);
    expect(s.scale).toBe(scaleBounds(image, view).max);
  });
  it("wheel up zooms in, wheel down zooms out", () => {
    expect(wheelFactor(-100)).toBeGreaterThan(1);
    expect(wheelFactor(100)).toBeLessThan(1);
  });
});

describe("pan bounds", () => {
  it("cannot drag the image off the edges", () => {
    const state = { scale: 1, x: 0, y: 0 };
    expect(panBy(state, 500, 500, image, view)).toEqual({ scale: 1, x: 0, y: 0 });
    const far = panBy(state, -5000, -5000, image, view);
    expect(far.x).toBe(800 - 1000);
    expect(far.y).toBe(600 - 2000);
  });
  it("keeps a smaller axis centred", () => {
    expect(clampPan(123, 0, 0.3, image, view).x).toBeCloseTo(250);
  });
});
