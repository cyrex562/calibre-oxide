// Column layout for the library table: resizing, reordering, picker order.
//
// Pure functions over keys and pixel numbers so every rule (a click vs
// a drag, where a dragged header lands, what "fit" means) is testable
// without a browser. BookTable.vue does the pointer plumbing only.
//
// # Why the table's width is the sum of its columns
//
// `table-layout: fixed` only honours column widths when the *table's*
// width is not `auto`. With no width set, the browser treated the table
// as auto-width, squeezed it into its container and shared the shortfall
// across the columns -- so a column could not be dragged wider than the
// room left over, and dragging one narrower grew the others back. Giving
// the table an explicit width of `tableWidth(...)` makes each column's
// width authoritative, and a table wider than its pane scrolls sideways.

import { clampWidth, DEFAULT_VISIBLE_KEYS, type BookColumn, type TablePrefs } from "./columns";

/** A pointer must move this far before a header press becomes a drag. */
export const DRAG_THRESHOLD = 4;

/** Whether a press that has moved by (dx, dy) is a drag rather than a click. */
export function isDrag(dx: number, dy: number, threshold = DRAG_THRESHOLD): boolean {
  return Math.hypot(dx, dy) >= threshold;
}

/** The width a resize drag produces: start width plus the pointer delta, clamped. */
export function resizedColumnWidth(startWidth: number, deltaX: number): number {
  return clampWidth(startWidth + deltaX);
}

/** Width of the check-box column shown in select mode. */
export const CHECK_COLUMN_WIDTH = 34;

/** The table's own width: the sum of its columns (see the module note). */
export function tableWidth(columns: { width: number }[], selectMode: boolean): number {
  return columns.reduce((sum, c) => sum + c.width, selectMode ? CHECK_COLUMN_WIDTH : 0);
}

/** Cell padding (both sides) plus a little air for the resize handle. */
export const AUTOFIT_PADDING = 28;
/** Room for the sort arrow in a header. */
export const SORT_ARROW_ROOM = 16;

/**
 * The width that fits a column's header and the cell texts given.
 * `measure` returns the pixel width of a string (a canvas in the
 * browser, a stub in tests).
 */
export function autoFitWidth(header: string, cells: string[], measure: (text: string) => number): number {
  let widest = measure(header) + SORT_ARROW_ROOM;
  for (const text of cells) widest = Math.max(widest, measure(text));
  return clampWidth(widest + AUTOFIT_PADDING);
}

/**
 * The insertion index (0..n, in the pre-move list) for a header dragged
 * to pointer position `x`, given each column header's left/right edges.
 * The pointer is compared with each column's midpoint.
 */
export function dropIndex(spans: { left: number; right: number }[], x: number): number {
  let index = 0;
  for (const s of spans) {
    if ((s.left + s.right) / 2 < x) index++;
    else break;
  }
  return index;
}

/**
 * Moves `key` so it lands before the item currently at `insertIndex`
 * (an index into the list as it is *before* the move, which is what
 * `dropIndex` returns). Dropping next to itself is a no-op.
 */
export function moveKeyTo(keys: string[], key: string, insertIndex: number): string[] {
  const from = keys.indexOf(key);
  if (from === -1) return keys;
  const target = Math.max(0, Math.min(keys.length, insertIndex));
  if (target === from || target === from + 1) return keys;
  const next = keys.filter((k) => k !== key);
  next.splice(target > from ? target - 1 : target, 0, key);
  return next;
}

/** Moves `key` one place left (-1) or right (+1). */
export function moveKeyBy(keys: string[], key: string, delta: number): string[] {
  const from = keys.indexOf(key);
  const to = from + delta;
  if (from === -1 || to < 0 || to >= keys.length) return keys;
  const next = [...keys];
  [next[from], next[to]] = [next[to], next[from]];
  return next;
}

/** Shows or hides a column. A newly shown column goes at the end. */
export function toggleKey(keys: string[], key: string): string[] {
  return keys.includes(key) ? keys.filter((k) => k !== key) : [...keys, key];
}

/**
 * The picker's rows: shown columns first in display order (so a
 * reorder is visible), then every hidden column in catalogue order.
 */
export function pickerOrder(available: BookColumn[], shownKeys: string[]): BookColumn[] {
  const byKey = new Map(available.map((c) => [c.key, c]));
  const shown = shownKeys.map((k) => byKey.get(k)).filter((c): c is BookColumn => c !== undefined);
  const shownSet = new Set(shown.map((c) => c.key));
  return [...shown, ...available.filter((c) => !shownSet.has(c.key))];
}

/** Default columns and widths, keeping the user's view mode. */
export function resetColumnPrefs(prefs: TablePrefs): TablePrefs {
  return { ...prefs, columns: [...DEFAULT_VISIBLE_KEYS], widths: {} };
}
