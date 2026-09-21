import { describe, expect, it } from "vitest";

import { adjustSpeed, autoScrollPixels, AUTO_SCROLL_SPEEDS, detectSwipe, SWIPE_MIN_DISTANCE, type Point } from "./gestures";

const at = (x: number, y: number, t = 0): Point => ({ x, y, t });

describe("detectSwipe", () => {
  it("recognises the four directions", () => {
    expect(detectSwipe(at(300, 100), at(100, 100, 200))).toBe("left");
    expect(detectSwipe(at(100, 100), at(300, 100, 200))).toBe("right");
    expect(detectSwipe(at(100, 300), at(100, 100, 200))).toBe("up");
    expect(detectSwipe(at(100, 100), at(100, 300, 200))).toBe("down");
  });

  // A finger never lands and lifts on exactly the same pixel. Without
  // a floor, every tap turns the page in whichever direction it
  // happened to drift.
  it("ignores the drift of a tap", () => {
    expect(detectSwipe(at(100, 100), at(103, 98, 60))).toBeNull();
    expect(detectSwipe(at(100, 100), at(100 + SWIPE_MIN_DISTANCE - 1, 100, 100))).toBeNull();
  });

  it("accepts exactly the minimum distance", () => {
    expect(detectSwipe(at(100, 100), at(100 + SWIPE_MIN_DISTANCE, 100, 100))).toBe("right");
  });

  // Without an axis check, scrolling down with a slight sideways lean
  // turns the page.
  it("ignores a diagonal drag", () => {
    expect(detectSwipe(at(100, 100), at(200, 190, 200))).toBeNull();
  });

  it("still recognises a mostly-horizontal swipe with some drift", () => {
    expect(detectSwipe(at(300, 100), at(100, 120, 200))).toBe("left");
  });

  // Someone moving a finger deliberately across the screen is not
  // asking to turn the page.
  it("ignores a slow drag", () => {
    expect(detectSwipe(at(300, 100), at(100, 100, 2000))).toBeNull();
  });

  // Two samples can carry the same timestamp; rejecting that would
  // drop a swipe that really happened.
  it("treats a zero duration as instantaneous rather than invalid", () => {
    expect(detectSwipe(at(300, 100, 5), at(100, 100, 5))).toBe("left");
  });
});

describe("autoScrollPixels", () => {
  it("scales with both speed and elapsed time", () => {
    const slow = autoScrollPixels(1, 100);
    expect(autoScrollPixels(2, 100)).toBeCloseTo(slow * 2);
    expect(autoScrollPixels(1, 200)).toBeCloseTo(slow * 2);
  });

  // Time-driven, so the same speed reads identically on a 60Hz and a
  // 120Hz display and a dropped frame does not slow the scroll.
  it("gives the same total distance however the time is divided", () => {
    const whole = autoScrollPixels(2, 1000);
    const split = autoScrollPixels(2, 500) + autoScrollPixels(2, 500);
    expect(split).toBeCloseTo(whole);
  });

  // Rounding per tick would make slow speeds round to zero and never
  // move at all, so this returns a float for the caller to accumulate.
  it("returns a sub-pixel amount for a slow speed and a short tick", () => {
    const px = autoScrollPixels(0.5, 16);
    expect(px).toBeGreaterThan(0);
    expect(px).toBeLessThan(1);
  });

  it("is zero for a stopped or nonsense speed", () => {
    expect(autoScrollPixels(0, 100)).toBe(0);
    expect(autoScrollPixels(-1, 100)).toBe(0);
    expect(autoScrollPixels(Number.NaN, 100)).toBe(0);
    expect(autoScrollPixels(1, 0)).toBe(0);
  });
});

describe("adjustSpeed", () => {
  it("steps up and down the ladder", () => {
    expect(adjustSpeed(1, 1)).toBe(1.5);
    expect(adjustSpeed(1.5, -1)).toBe(1);
  });

  it("clamps at both ends", () => {
    expect(adjustSpeed(AUTO_SCROLL_SPEEDS[0], -1)).toBe(AUTO_SCROLL_SPEEDS[0]);
    expect(adjustSpeed(AUTO_SCROLL_SPEEDS[AUTO_SCROLL_SPEEDS.length - 1], 1)).toBe(AUTO_SCROLL_SPEEDS[AUTO_SCROLL_SPEEDS.length - 1]);
  });

  // A speed restored from storage need not sit exactly on the ladder.
  it("snaps an off-ladder speed to the nearest step first", () => {
    expect(adjustSpeed(1.4, 1)).toBe(2);
    expect(adjustSpeed(1.4, 0)).toBe(1.5);
  });
});
