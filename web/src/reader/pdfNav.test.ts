import { beforeEach, describe, expect, it } from "vitest";

import { MAX_SCALE, MIN_SCALE } from "./pdf";
import { bookmarkPage, DEFAULT_ZOOM, fitScale, flattenOutline, loadZoom, resolveDestPage, saveZoom } from "./pdfNav";

describe("fitScale", () => {
  it("fits the width, and the page by its tighter side", () => {
    // A 600x800 page in a 900x600 window.
    expect(fitScale("width", 600, 800, 900, 600)).toBeCloseTo(1.5);
    expect(fitScale("page", 600, 800, 900, 600)).toBeCloseTo(0.75);
  });

  it("keeps a custom scale and never leaves the zoom bounds", () => {
    expect(fitScale("custom", 600, 800, 900, 600, 2)).toBe(2);
    expect(fitScale("width", 10, 10, 100000, 100)).toBe(MAX_SCALE);
    expect(fitScale("width", 100000, 10, 100, 100)).toBe(MIN_SCALE);
  });

  it("falls back to the current scale before the area has a size", () => {
    expect(fitScale("width", 600, 800, 0, 0, 1.25)).toBe(1.25);
  });
});

describe("zoom persistence", () => {
  beforeEach(() => localStorage.clear());

  it("defaults to fit width", () => {
    expect(loadZoom()).toEqual(DEFAULT_ZOOM);
    expect(DEFAULT_ZOOM.mode).toBe("width");
  });

  it("remembers the last choice", () => {
    saveZoom({ mode: "custom", scale: 1.75 });
    expect(loadZoom()).toEqual({ mode: "custom", scale: 1.75 });
    saveZoom({ mode: "page", scale: 1 });
    expect(loadZoom().mode).toBe("page");
  });

  it("ignores a damaged entry", () => {
    localStorage.setItem("lexmechanic.pdf.zoom", "{nope");
    expect(loadZoom()).toEqual(DEFAULT_ZOOM);
    localStorage.setItem("lexmechanic.pdf.zoom", JSON.stringify({ mode: "sideways" }));
    expect(loadZoom()).toEqual(DEFAULT_ZOOM);
  });
});

describe("outline", () => {
  it("flattens nested entries with their depth", () => {
    const rows = flattenOutline([
      { title: "One", dest: "a", items: [{ title: "One.1", dest: "b", items: [] }] },
      { title: "  ", dest: null },
    ]);
    expect(rows.map((r) => [r.title, r.depth])).toEqual([
      ["One", 0],
      ["One.1", 1],
      ["(untitled)", 0],
    ]);
  });

  const doc = {
    getDestination: async (name: string) => (name === "chap2" ? [{ num: 7, gen: 0 }, { name: "XYZ" }] : null),
    getPageIndex: async (ref: { num: number }) => ref.num - 5,
  };

  it("resolves a named destination to a 1-based page", async () => {
    expect(await resolveDestPage(doc as never, "chap2")).toBe(3);
  });

  it("resolves an explicit destination, by reference or by index", async () => {
    expect(await resolveDestPage(doc as never, [{ num: 5, gen: 0 }])).toBe(1);
    expect(await resolveDestPage(doc as never, [4, { name: "Fit" }])).toBe(5);
  });

  it("gives null for a dangling or missing destination", async () => {
    expect(await resolveDestPage(doc as never, "nowhere")).toBeNull();
    expect(await resolveDestPage(doc as never, null)).toBeNull();
    const broken = { getDestination: async () => Promise.reject(new Error("x")), getPageIndex: async () => 0 };
    expect(await resolveDestPage(broken as never, "a")).toBeNull();
  });
});

describe("bookmarkPage", () => {
  it("reads the stored page and rejects nonsense", () => {
    expect(bookmarkPage("12", 40)).toBe(12);
    expect(bookmarkPage("41", 40)).toBeNull();
    expect(bookmarkPage("0", 40)).toBeNull();
    expect(bookmarkPage("abc", 40)).toBeNull();
  });
});
