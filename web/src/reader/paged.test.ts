import { describe, expect, it } from "vitest";

import { clampPageIndex, COLUMN_GAP, pageCount, pageForScroll, pagedModeCss, pageStride, scrollForPage } from "./paged";

const W = 800;
const STRIDE = W + COLUMN_GAP;

describe("pageStride", () => {
  // The gap sits between columns, so a page turn moves by both.
  it("is a viewport plus a gap", () => {
    expect(pageStride(W)).toBe(STRIDE);
  });

  it("never collapses to zero", () => {
    expect(pageStride(0)).toBeGreaterThan(0);
    expect(pageStride(-10)).toBeGreaterThan(0);
  });
});

describe("pageCount", () => {
  // `scrollWidth` counts the gaps *between* columns but not a
  // trailing one. Without adding it back, content that exactly fills
  // N columns reports N-1 pages and the last page is unreachable.
  it("counts a section that exactly fills whole columns", () => {
    expect(pageCount(W, W)).toBe(1);
    expect(pageCount(2 * W + COLUMN_GAP, W)).toBe(2);
    expect(pageCount(3 * W + 2 * COLUMN_GAP, W)).toBe(3);
  });

  // Rounding to *nearest* loses a final column that is less than
  // half full -- which is most of them -- and the reader can never
  // turn to the end of the section.
  it("rounds a partial final column up to a page", () => {
    expect(pageCount(W + COLUMN_GAP + 100, W)).toBe(2);
    expect(pageCount(W + COLUMN_GAP + 1, W)).toBe(2);
    expect(pageCount(2 * W + 2 * COLUMN_GAP + 400, W)).toBe(3);
  });

  // Sub-pixel layout makes `scrollWidth` fractional; without a guard
  // an exact fit would round up to a spurious blank page.
  it("does not invent a blank page for a sub-pixel overflow", () => {
    expect(pageCount(2 * W + COLUMN_GAP + 0.4, W)).toBe(2);
  });

  it("is at least one page even when empty", () => {
    expect(pageCount(0, W)).toBe(1);
    expect(pageCount(-5, W)).toBe(1);
    expect(pageCount(W, 0)).toBe(1);
  });
});

describe("scrollForPage and pageForScroll", () => {
  it("round-trip for every page of a section", () => {
    for (let page = 0; page < 6; page += 1) {
      expect(pageForScroll(scrollForPage(page, W), W)).toBe(page);
    }
  });

  it("starts at zero", () => {
    expect(scrollForPage(0, W)).toBe(0);
    expect(pageForScroll(0, W)).toBe(0);
    expect(pageForScroll(-20, W)).toBe(0);
  });

  // Smooth scrolling and sub-pixel layout both land a pixel or two
  // short of a boundary; truncating would report the previous page
  // and the reader would appear stuck.
  it("treats a position a pixel short of a boundary as that page", () => {
    expect(pageForScroll(STRIDE - 1, W)).toBe(1);
    expect(pageForScroll(2 * STRIDE - 2, W)).toBe(2);
  });

  it("treats a position a pixel past a boundary as that page", () => {
    expect(pageForScroll(STRIDE + 1, W)).toBe(1);
  });

  it("never returns a negative page", () => {
    expect(scrollForPage(-3, W)).toBe(0);
  });
});

describe("clampPageIndex", () => {
  it("keeps an index inside the section", () => {
    expect(clampPageIndex(-1, 5)).toBe(0);
    expect(clampPageIndex(9, 5)).toBe(4);
    expect(clampPageIndex(2, 5)).toBe(2);
  });

  it("copes with a single-page section", () => {
    expect(clampPageIndex(3, 1)).toBe(0);
  });

  it("survives a corrupt stored index", () => {
    expect(clampPageIndex(Number.NaN, 5)).toBe(0);
  });
});

describe("pagedModeCss", () => {
  const css = pagedModeCss(800, 600);

  // Multi-column needs a bounded height to know where to break a
  // column. With `auto` the content stays in one very tall column and
  // nothing paginates at all.
  it("fixes the height so columns can break", () => {
    expect(css).toContain("height: 600px");
    expect(css).not.toContain("height: auto");
  });

  it("sets a column width of one viewport", () => {
    expect(css).toContain("column-width: 800px");
    expect(css).toContain(`column-gap: ${COLUMN_GAP}px`);
  });

  // Otherwise the browser offers its own scrollbar for content now
  // laid out sideways, and the reader has two competing scroll
  // mechanisms.
  it("hides the browser's own overflow", () => {
    expect(css).toContain("overflow: hidden");
  });

  // An image taller than the page creates a column of its own that
  // nothing can scroll to.
  it("bounds media to the page", () => {
    expect(css).toContain("max-height: 600px");
  });

  it("uses !important so a book's own stylesheet cannot undo it", () => {
    // Books routinely set their own body margins and overflow.
    expect(css.match(/!important/g)?.length ?? 0).toBeGreaterThan(5);
  });
});
