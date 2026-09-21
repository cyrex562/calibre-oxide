import { describe, expect, it } from "vitest";

import { clampPage, clampScale, MAX_SCALE, MIN_SCALE, pdfUrl } from "./pdf";

describe("clampPage", () => {
  // Pages are 1-based. A 0 or an n+1 reaches PDF.js as a *rejected
  // promise* rather than a clamped page, so the reader would show an
  // error where the user expects to simply be at an edge.
  it("keeps a page inside the document", () => {
    expect(clampPage(0, 10)).toBe(1);
    expect(clampPage(11, 10)).toBe(10);
    expect(clampPage(5, 10)).toBe(5);
  });

  it("clamps both ends at a single-page document", () => {
    expect(clampPage(0, 1)).toBe(1);
    expect(clampPage(99, 1)).toBe(1);
  });

  it("floors a fractional page", () => {
    expect(clampPage(3.7, 10)).toBe(3);
  });

  // A stored position from a previous session is parsed from text and
  // can be anything. Any non-finite value means "no usable stored
  // position", so the reader opens at the beginning -- deliberately
  // not clamped to the last page, since Infinity is not "past the
  // end", it is garbage, and dumping someone at the end of a 400-page
  // PDF is a worse answer than the start.
  it("falls back to the first page for a corrupt stored position", () => {
    expect(clampPage(Number.NaN, 10)).toBe(1);
    expect(clampPage(Number.POSITIVE_INFINITY, 10)).toBe(1);
    expect(clampPage(Number.NEGATIVE_INFINITY, 10)).toBe(1);
  });

  it("returns page 1 before the page count is known", () => {
    expect(clampPage(5, 0)).toBe(1);
  });
});

describe("clampScale", () => {
  it("keeps zoom usable at both extremes", () => {
    expect(clampScale(0.01)).toBe(MIN_SCALE);
    expect(clampScale(100)).toBe(MAX_SCALE);
    expect(clampScale(1.5)).toBe(1.5);
  });

  it("falls back to 1 for a nonsense scale", () => {
    expect(clampScale(Number.NaN)).toBe(1);
  });
});

describe("pdfUrl", () => {
  // PDF.js reads the file straight from the content route -- there is
  // no manifest or render step for a PDF, unlike EPUB.
  it("points at the content route", () => {
    expect(pdfUrl("42")).toBe("/get/pdf/42");
  });
});
