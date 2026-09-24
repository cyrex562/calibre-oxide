// Panel layout for the library window (calibre-shaped three columns).
//
// calibre's window is categories | book list | details, over a status
// bar, with draggable splitters between the columns. The parts that
// are easy to get subtly wrong -- clamping, reading back a stored blob
// that may be from an older build, and which direction each splitter
// grows in -- live here so they can be tested without a browser.

export const SIDEBAR_MIN = 140;
export const SIDEBAR_MAX = 520;
export const DETAILS_MIN = 240;
export const DETAILS_MAX = 720;

export const LAYOUT_KEY = "calibre-oxide.layout";

export interface LayoutPrefs {
  sidebarVisible: boolean;
  detailsVisible: boolean;
  sidebarWidth: number;
  detailsWidth: number;
}

export const LAYOUT_DEFAULTS: LayoutPrefs = {
  sidebarVisible: true,
  detailsVisible: true,
  sidebarWidth: 240,
  detailsWidth: 360,
};

export function clamp(value: number, lo: number, hi: number): number {
  if (!Number.isFinite(value)) return lo;
  return Math.min(hi, Math.max(lo, value));
}

export type Panel = "sidebar" | "details";

export function widthBounds(panel: Panel): [number, number] {
  return panel === "sidebar" ? [SIDEBAR_MIN, SIDEBAR_MAX] : [DETAILS_MIN, DETAILS_MAX];
}

/**
 * The width a splitter drag should produce.
 *
 * The sidebar sits left of its handle and grows as the pointer moves
 * right. The details panel sits *right* of its handle, so the same
 * rightward movement makes it narrower -- the delta is inverted.
 * Getting this backwards yields a panel that shrinks when dragged
 * outward, which feels broken rather than merely wrong.
 */
export function resizedWidth(panel: Panel, startWidth: number, startX: number, currentX: number): number {
  const delta = panel === "sidebar" ? currentX - startX : startX - currentX;
  const [lo, hi] = widthBounds(panel);
  return clamp(startWidth + delta, lo, hi);
}

/**
 * Parses stored layout, falling back per-field.
 *
 * Every field is validated separately rather than trusting the blob:
 * it can come from an older build with different bounds, or from a
 * hand-edited localStorage, and a panel restored wider than the window
 * cannot be dragged back into reach.
 */
export function parseLayout(raw: string | null): LayoutPrefs {
  if (!raw) return { ...LAYOUT_DEFAULTS };
  let parsed: Partial<LayoutPrefs>;
  try {
    parsed = JSON.parse(raw) as Partial<LayoutPrefs>;
  } catch {
    return { ...LAYOUT_DEFAULTS };
  }
  if (typeof parsed !== "object" || parsed === null) return { ...LAYOUT_DEFAULTS };
  return {
    sidebarVisible: typeof parsed.sidebarVisible === "boolean" ? parsed.sidebarVisible : LAYOUT_DEFAULTS.sidebarVisible,
    detailsVisible: typeof parsed.detailsVisible === "boolean" ? parsed.detailsVisible : LAYOUT_DEFAULTS.detailsVisible,
    sidebarWidth: clamp(toNumber(parsed.sidebarWidth, LAYOUT_DEFAULTS.sidebarWidth), SIDEBAR_MIN, SIDEBAR_MAX),
    detailsWidth: clamp(toNumber(parsed.detailsWidth, LAYOUT_DEFAULTS.detailsWidth), DETAILS_MIN, DETAILS_MAX),
  };
}

function toNumber(value: unknown, fallback: number): number {
  const n = typeof value === "number" ? value : Number(value);
  return Number.isFinite(n) ? n : fallback;
}
