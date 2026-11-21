// Pure windowing maths for the library's continuous list.
//
// The library shows every matching book in one scrolling list, which
// at 50,000 books cannot be 50,000 DOM nodes. These functions answer
// "which rows are on screen" from a scroll position, so the table and
// grid components render only that slice between two spacers and the
// scrollbar still represents the whole list. Nothing here touches the
// DOM, which is what makes it testable.

export interface WindowInput {
  /** Total number of rows in the list. */
  count: number;
  /** Fixed height of one row, in pixels. */
  rowHeight: number;
  /** How far the scroll container is scrolled, in pixels. */
  scrollTop: number;
  /** Height of the visible area for rows, in pixels. */
  viewportHeight: number;
  /** Extra rows rendered above and below the visible ones. */
  overscan?: number;
}

export interface WindowRange {
  /** First row at least partly on screen (inclusive). */
  first: number;
  /** One past the last row at least partly on screen. */
  last: number;
  /** First row to render, overscan included (inclusive). */
  start: number;
  /** One past the last row to render, overscan included. */
  end: number;
  /** Height of the spacer above the first rendered row. */
  padTop: number;
  /** Height of the spacer below the last rendered row. */
  padBottom: number;
  /** Full height of the list. */
  totalHeight: number;
}

export const DEFAULT_OVERSCAN = 8;

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

/** The slice of rows to render for a scroll position. Always clamped to `[0, count]`. */
export function windowRange(input: WindowInput): WindowRange {
  const count = Math.max(0, Math.floor(input.count));
  const rowHeight = Math.max(1, input.rowHeight);
  const overscan = Math.max(0, input.overscan ?? DEFAULT_OVERSCAN);
  const totalHeight = count * rowHeight;
  const viewport = Math.max(0, input.viewportHeight);
  // A list can shrink while scrolled (a refresh with fewer matches);
  // keep the position inside the new height.
  const scrollTop = clamp(Number.isFinite(input.scrollTop) ? input.scrollTop : 0, 0, Math.max(0, totalHeight - viewport));

  const first = clamp(Math.floor(scrollTop / rowHeight), 0, count);
  const last = clamp(Math.ceil((scrollTop + viewport) / rowHeight), first, count);
  const start = clamp(first - overscan, 0, count);
  const end = clamp(last + overscan, start, count);
  return { first, last, start, end, padTop: start * rowHeight, padBottom: (count - end) * rowHeight, totalHeight };
}

/**
 * The scrollTop that brings row `index` fully into view with the least
 * movement: unchanged when it is already visible.
 */
export function scrollTopForIndex(index: number, input: Pick<WindowInput, "count" | "rowHeight" | "scrollTop" | "viewportHeight">): number {
  const count = Math.max(0, Math.floor(input.count));
  if (count === 0) return 0;
  const rowHeight = Math.max(1, input.rowHeight);
  const i = clamp(Math.floor(index), 0, count - 1);
  const viewport = Math.max(0, input.viewportHeight);
  const top = i * rowHeight;
  const bottom = top + rowHeight;
  const maxScroll = Math.max(0, count * rowHeight - viewport);
  let next = input.scrollTop;
  if (top < input.scrollTop) next = top;
  else if (bottom > input.scrollTop + viewport) next = bottom - viewport;
  return clamp(next, 0, maxScroll);
}

/**
 * The index a navigation key moves to from `current` (-1 when nothing
 * is selected), clamped to the list. `pageSize` is how many rows fit.
 */
export function navigateIndex(key: string, current: number, count: number, pageSize: number): number | null {
  if (count <= 0) return null;
  const page = Math.max(1, pageSize - 1);
  const from = current < 0 ? 0 : current;
  switch (key) {
    case "ArrowDown":
      return current < 0 ? 0 : clamp(current + 1, 0, count - 1);
    case "ArrowUp":
      return current < 0 ? 0 : clamp(current - 1, 0, count - 1);
    case "PageDown":
      return clamp(from + page, 0, count - 1);
    case "PageUp":
      return clamp(from - page, 0, count - 1);
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

// ---------------------------------------------------------------
// Grid
// ---------------------------------------------------------------

export interface GridMetrics {
  /** Cards per row. */
  columns: number;
  /** Width of one card. */
  cardWidth: number;
  /** Height of a row of cards, gap below included. */
  rowHeight: number;
  /** Number of rows needed for `count` cards. */
  rows: number;
}

export interface GridOptions {
  /** Narrowest a card may get. */
  minCardWidth: number;
  gap: number;
  /** Cover height as a multiple of card width. */
  coverRatio: number;
  /** Height reserved under the cover for title and authors. */
  captionHeight: number;
}

export const DEFAULT_GRID: GridOptions = { minCardWidth: 120, gap: 16, coverRatio: 1.5, captionHeight: 48 };

/** Cards per row, card size and row height from the container width. */
export function gridMetrics(width: number, count: number, options: GridOptions = DEFAULT_GRID): GridMetrics {
  const w = Math.max(0, Number.isFinite(width) ? width : 0);
  const columns = Math.max(1, Math.floor((w + options.gap) / (options.minCardWidth + options.gap)));
  const cardWidth = Math.max(1, (w - (columns - 1) * options.gap) / columns);
  const rowHeight = Math.round(cardWidth * options.coverRatio + options.captionHeight + options.gap);
  return { columns, cardWidth, rowHeight, rows: Math.ceil(Math.max(0, count) / columns) };
}

/** Book indexes `[from, to)` covered by grid rows `[startRow, endRow)`, clamped to `count`. */
export function gridRowItems(startRow: number, endRow: number, columns: number, count: number): { from: number; to: number } {
  const from = clamp(startRow * columns, 0, count);
  const to = clamp(endRow * columns, from, count);
  return { from, to };
}

/** The grid row that holds book `index`. */
export function gridRowOf(index: number, columns: number): number {
  return Math.floor(Math.max(0, index) / Math.max(1, columns));
}
