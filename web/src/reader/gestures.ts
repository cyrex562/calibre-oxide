// Touch gestures and auto-scroll pacing (issues 2.7 and 2.8 of the
// #816 epic).
//
// The reader was mouse-and-keyboard only: on a tablet or a
// touchscreen laptop there was no way to turn a page at all.
//
// # Why the geometry is pure
//
// Swipe recognition is a pile of thresholds, and every one of them is
// a judgement that is wrong in a specific, reproducible way: too
// small and a tap that drifts two pixels turns the page, too large
// and a real swipe does nothing, no angle check and scrolling
// vertically flips pages sideways. None of that is observable by
// reading the code, and none of it needs a browser to test.

/** A touch point, reduced to what gesture recognition needs. */
export interface Point {
  x: number;
  y: number;
  /** Milliseconds since some fixed origin. */
  t: number;
}

export type SwipeDirection = "left" | "right" | "up" | "down";

/**
 * Minimum travel before a drag counts as a swipe.
 *
 * A finger never lands and lifts at exactly the same pixel, so
 * without a floor every tap registers as a swipe in whichever
 * direction the finger happened to drift.
 */
export const SWIPE_MIN_DISTANCE = 50;

/**
 * A swipe has to be mostly along one axis. Without this, scrolling
 * down a page with a slight sideways lean turns the page.
 */
export const SWIPE_AXIS_RATIO = 1.7;

/**
 * Slower than this is a drag, not a swipe -- someone moving a finger
 * deliberately across the screen is not asking to turn the page.
 */
export const SWIPE_MAX_DURATION = 800;

/**
 * The direction of a swipe, or `null` when the movement was not one.
 */
export function detectSwipe(start: Point, end: Point): SwipeDirection | null {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const duration = end.t - start.t;

  // A non-positive duration means the clock did not advance between
  // the two samples; treat it as instantaneous rather than rejecting
  // a swipe that really happened.
  if (duration > SWIPE_MAX_DURATION) return null;

  const absX = Math.abs(dx);
  const absY = Math.abs(dy);

  if (absX >= SWIPE_MIN_DISTANCE && absX >= absY * SWIPE_AXIS_RATIO) {
    return dx < 0 ? "left" : "right";
  }
  if (absY >= SWIPE_MIN_DISTANCE && absY >= absX * SWIPE_AXIS_RATIO) {
    return dy < 0 ? "up" : "down";
  }
  return null;
}

// ---------------------------------------------------------------
// Auto-scroll
// ---------------------------------------------------------------

/** Speed steps, in lines per second, from a crawl to a skim. */
export const AUTO_SCROLL_SPEEDS = [0.5, 1, 1.5, 2, 3, 4, 6] as const;

export const DEFAULT_AUTO_SCROLL_SPEED = 1.5;

/** Assumed line height when converting a speed into pixels. */
const LINE_HEIGHT_PX = 24;

/**
 * Pixels to scroll for a tick of `elapsedMs`.
 *
 * Driven by elapsed time rather than a fixed step per frame so the
 * speed is the same on a 60Hz and a 120Hz display, and so a dropped
 * frame does not slow the scroll down.
 *
 * Returns a float; the caller accumulates the remainder, because
 * rounding each tick to a whole pixel would make slow speeds round to
 * zero and never move at all.
 */
export function autoScrollPixels(linesPerSecond: number, elapsedMs: number): number {
  if (!Number.isFinite(linesPerSecond) || linesPerSecond <= 0) return 0;
  if (!Number.isFinite(elapsedMs) || elapsedMs <= 0) return 0;
  return (linesPerSecond * LINE_HEIGHT_PX * elapsedMs) / 1000;
}

/** The next speed up or down the ladder, clamped at both ends. */
export function adjustSpeed(current: number, delta: number): number {
  const speeds = AUTO_SCROLL_SPEEDS;
  // Nearest step, so a stored speed that is not exactly on the ladder
  // still moves predictably.
  let index = 0;
  let best = Number.POSITIVE_INFINITY;
  for (let i = 0; i < speeds.length; i += 1) {
    const d = Math.abs(speeds[i] - current);
    if (d < best) {
      best = d;
      index = i;
    }
  }
  const next = Math.min(speeds.length - 1, Math.max(0, index + delta));
  return speeds[next];
}
