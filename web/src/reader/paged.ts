// Paged reading mode (issue 2.3 of the #816 epic).
//
// The reader only ever scrolled. Paged mode is what most people mean
// by reading an ebook: fixed pages that turn, rather than a
// continuous ribbon of text.
//
// # How it works
//
// The content is laid out with CSS multi-column at exactly the
// viewport width, so it flows into side-by-side columns. Turning a
// page is then a horizontal scroll by one column plus its gap -- no
// re-layout, no measuring text, and the book's own stylesheet still
// applies.
//
// # Why the arithmetic is separate
//
// Every value here is an off-by-one waiting to happen: the gap counts
// between columns but not after the last one, the final page is
// usually partial, and a rounding error puts the reader half a page
// off with no visible cause. That is worth stating in one place and
// testing directly, rather than rediscovering inside a component that
// needs a browser to run.

/** Gap between columns, in CSS pixels. */
export const COLUMN_GAP = 48;

/**
 * The horizontal distance from the start of one page to the next.
 *
 * A column is one viewport wide and the gap sits *between* columns,
 * so a page turn moves by both.
 */
export function pageStride(viewportWidth: number): number {
  return Math.max(1, viewportWidth) + COLUMN_GAP;
}

/**
 * How many pages a section occupies.
 *
 * `scrollWidth` covers every column plus the gaps between them but
 * not a trailing one, so the final gap is added back before dividing
 * -- without it, a section whose content exactly fills N columns
 * reports N-1 pages and the last page becomes unreachable.
 *
 * Rounds *up*, not to nearest: a partial final column is still a page
 * the reader has to be able to turn to. Rounding to nearest loses it
 * whenever it is less than half full, which is most of the time.
 *
 * The epsilon absorbs sub-pixel `scrollWidth` values, which would
 * otherwise turn an exact fit into a spurious extra blank page. It is
 * far smaller than a pixel, so it cannot swallow a real overflow.
 */
export function pageCount(scrollWidth: number, viewportWidth: number): number {
  if (scrollWidth <= 0 || viewportWidth <= 0) return 1;
  const stride = pageStride(viewportWidth);
  return Math.max(1, Math.ceil((scrollWidth + COLUMN_GAP) / stride - 0.001));
}

/** Where to scroll so that `page` (0-based) is on screen. */
export function scrollForPage(page: number, viewportWidth: number): number {
  return Math.max(0, page) * pageStride(viewportWidth);
}

/**
 * Which page a scroll position is showing.
 *
 * Rounds rather than truncates: a scroll that lands a pixel short of
 * a boundary -- which smooth scrolling and sub-pixel layout both
 * produce -- is showing that page, not the one before it.
 */
export function pageForScroll(scrollLeft: number, viewportWidth: number): number {
  if (scrollLeft <= 0) return 0;
  return Math.max(0, Math.round(scrollLeft / pageStride(viewportWidth)));
}

/** Keeps a page index inside a section. */
export function clampPageIndex(page: number, total: number): number {
  if (!Number.isFinite(page)) return 0;
  return Math.min(Math.max(0, total - 1), Math.max(0, Math.floor(page)));
}

/**
 * The stylesheet that turns a scrolling document into a paged one.
 *
 * `overflow: hidden` on the root is what stops the browser offering
 * its own scrollbar for content that is now laid out sideways;
 * movement happens through `scrollLeft` instead.
 *
 * `height` is fixed rather than `auto` because multi-column layout
 * needs a bounded height to know where to break a column -- with
 * `auto` the content stays in one very tall column and nothing
 * paginates.
 */
export function pagedModeCss(viewportWidth: number, viewportHeight: number): string {
  return `
html {
  overflow: hidden !important;
  height: ${viewportHeight}px !important;
}
body {
  margin: 0 !important;
  padding: 0 1em !important;
  box-sizing: border-box !important;
  height: ${viewportHeight}px !important;
  column-width: ${Math.max(1, viewportWidth)}px !important;
  column-gap: ${COLUMN_GAP}px !important;
  column-fill: auto !important;
  overflow: hidden !important;
}
/* An image taller than the page would otherwise create a column of
   its own that nothing can scroll to. */
img, svg, video {
  max-width: 100% !important;
  max-height: ${viewportHeight}px !important;
}
`.trim();
}

/** Undoes `pagedModeCss` when returning to scrolling mode. */
export const SCROLLING_MODE_CSS = "";
