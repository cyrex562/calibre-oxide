import { describe, expect, it } from "vitest";

import { clampPage, clampScale, imageMimeType, MAX_SCALE, MIN_SCALE, pageImageFileName, pdfUrl, scaleForWidth } from "./pdf";

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

describe("scaleForWidth", () => {
  // A cover should be the same size whatever the reader is zoomed to.
  it("scales a page to the requested pixel width", () => {
    expect(scaleForWidth(612, 1000)).toBeCloseTo(1000 / 612);
    expect(scaleForWidth(1000, 1000)).toBe(1);
  });

  // PDF.js can hand back a zero-width viewport for a malformed page.
  // Falling back to 1 renders something; dividing by it would produce
  // an Infinity scale and a canvas allocation the size of the heap.
  it("falls back to 1:1 rather than dividing by a nonsense width", () => {
    expect(scaleForWidth(0, 1000)).toBe(1);
    expect(scaleForWidth(-5, 1000)).toBe(1);
    expect(scaleForWidth(Number.NaN, 1000)).toBe(1);
    expect(scaleForWidth(612, 0)).toBe(1);
    expect(scaleForWidth(612, Number.POSITIVE_INFINITY)).toBe(1);
  });
});

describe("pageImageFileName", () => {
  it("names the file after the book and the page", () => {
    expect(pageImageFileName("Dune", 3, "jpg")).toBe("Dune - page 3.jpg");
    expect(pageImageFileName("Dune", 3, "png")).toBe("Dune - page 3.png");
  });

  // Both separators: this app runs on Windows too, where a backslash
  // in a suggested download name is a path, not a character.
  it("strips characters that would make it a path", () => {
    expect(pageImageFileName('A/B\\C"D', 1, "png")).toBe("A_B_C_D - page 1.png");
  });

  it("keeps the name short enough to save", () => {
    const name = pageImageFileName("x".repeat(200), 1, "jpg");
    expect(name.startsWith("x".repeat(60))).toBe(true);
    expect(name).toBe(`${"x".repeat(60)} - page 1.jpg`);
  });

  it("falls back to a name when the book has no title", () => {
    expect(pageImageFileName("", 2, "jpg")).toBe("book - page 2.jpg");
    // A title that is nothing but separators sanitizes to underscores,
    // which is still a usable name -- but whitespace alone does not.
    expect(pageImageFileName("   ", 2, "jpg")).toBe("book - page 2.jpg");
  });
});

describe("imageMimeType", () => {
  it("maps the extension the UI offers to the type canvas encodes", () => {
    expect(imageMimeType("jpg")).toBe("image/jpeg");
    expect(imageMimeType("png")).toBe("image/png");
  });
});
