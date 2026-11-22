// Zoom and pan maths for the full-window image viewer.
//
// The image is drawn at `scale` with its top-left at (x, y) inside a
// viewport. Everything here is pure so fit / 100% / clamped zoom / pan
// bounds can be tested without a browser.

export interface Size {
  w: number;
  h: number;
}

export interface ViewState {
  scale: number;
  x: number;
  y: number;
}

/** The scale at which the whole image fits the viewport. */
export function fitScale(image: Size, view: Size): number {
  if (image.w <= 0 || image.h <= 0 || view.w <= 0 || view.h <= 0) return 1;
  return Math.min(view.w / image.w, view.h / image.h);
}

/** Zoom limits: down to half the fit size (or half of 100% for a small image), up to 8x (or 8x the fit, if larger). */
export function scaleBounds(image: Size, view: Size): { min: number; max: number } {
  const fit = fitScale(image, view);
  return { min: Math.min(fit, 1) * 0.5, max: Math.max(8, fit * 8) };
}

export function clampScale(scale: number, image: Size, view: Size): number {
  const { min, max } = scaleBounds(image, view);
  if (!Number.isFinite(scale)) return fitScale(image, view);
  return Math.min(max, Math.max(min, scale));
}

/**
 * Keeps the image from being panned off-screen: an image smaller than
 * the viewport on an axis is centred on it, a larger one may not leave
 * a gap at either edge.
 */
export function clampPan(x: number, y: number, scale: number, image: Size, view: Size): { x: number; y: number } {
  const cw = image.w * scale;
  const ch = image.h * scale;
  return {
    x: cw <= view.w ? (view.w - cw) / 2 : Math.min(0, Math.max(view.w - cw, x)),
    y: ch <= view.h ? (view.h - ch) / 2 : Math.min(0, Math.max(view.h - ch, y)),
  };
}

/** The whole image, centred. */
export function fitState(image: Size, view: Size): ViewState {
  const scale = fitScale(image, view);
  return { scale, ...clampPan(0, 0, scale, image, view) };
}

/** One image pixel per screen pixel, centred on the viewport. */
export function actualSizeState(image: Size, view: Size): ViewState {
  const scale = clampScale(1, image, view);
  return { scale, ...clampPan(0, 0, scale, image, view) };
}

/** Zooms by `factor` keeping the image point under (ax, ay) fixed. */
export function zoomAt(state: ViewState, factor: number, ax: number, ay: number, image: Size, view: Size): ViewState {
  const scale = clampScale(state.scale * factor, image, view);
  const ratio = scale / state.scale;
  const pan = clampPan(ax - (ax - state.x) * ratio, ay - (ay - state.y) * ratio, scale, image, view);
  return { scale, ...pan };
}

/** Pans by a pointer delta, within bounds. */
export function panBy(state: ViewState, dx: number, dy: number, image: Size, view: Size): ViewState {
  return { scale: state.scale, ...clampPan(state.x + dx, state.y + dy, state.scale, image, view) };
}

/** The zoom factor one wheel notch applies (deltaY > 0 zooms out). */
export function wheelFactor(deltaY: number): number {
  return Math.exp(-Math.max(-400, Math.min(400, deltaY)) * 0.0015);
}
