import { describe, expect, it } from "vitest";

import { gridMetrics, gridRowItems, gridRowOf, navigateIndex, scrollTopForIndex, windowRange } from "./virtualList";

describe("windowRange", () => {
  const base = { count: 1000, rowHeight: 30, viewportHeight: 300, overscan: 5 };

  it("starts at the top with overscan only below", () => {
    const r = windowRange({ ...base, scrollTop: 0 });
    expect(r.first).toBe(0);
    expect(r.last).toBe(10);
    expect(r.start).toBe(0);
    expect(r.end).toBe(15);
    expect(r.padTop).toBe(0);
    expect(r.padBottom).toBe((1000 - 15) * 30);
    expect(r.totalHeight).toBe(30000);
  });

  it("finds the first and last visible rows mid-list, with overscan both sides", () => {
    const r = windowRange({ ...base, scrollTop: 3000 + 10 }); // row 100, 10px into it
    expect(r.first).toBe(100);
    expect(r.last).toBe(111); // partly visible row 110 counts
    expect(r.start).toBe(95);
    expect(r.end).toBe(116);
    expect(r.padTop).toBe(95 * 30);
  });

  it("clamps at the bottom", () => {
    const r = windowRange({ ...base, scrollTop: 1e9 });
    expect(r.last).toBe(1000);
    expect(r.end).toBe(1000);
    expect(r.padBottom).toBe(0);
    expect(r.first).toBe(990);
  });

  it("clamps a negative or NaN scroll to the top", () => {
    expect(windowRange({ ...base, scrollTop: -50 }).first).toBe(0);
    expect(windowRange({ ...base, scrollTop: NaN }).first).toBe(0);
  });

  it("handles an empty list and a list shorter than the viewport", () => {
    const empty = windowRange({ ...base, count: 0, scrollTop: 400 });
    expect([empty.start, empty.end, empty.padTop, empty.padBottom]).toEqual([0, 0, 0, 0]);
    const short = windowRange({ ...base, count: 4, scrollTop: 0 });
    expect([short.first, short.last, short.end]).toEqual([0, 4, 4]);
  });

  it("follows a resize: a taller viewport shows more rows", () => {
    const small = windowRange({ ...base, scrollTop: 600 });
    const tall = windowRange({ ...base, viewportHeight: 900, scrollTop: 600 });
    expect(tall.last - tall.first).toBeGreaterThan(small.last - small.first);
    expect(tall.first).toBe(small.first);
  });

  it("keeps the position inside a list that shrank while scrolled", () => {
    const r = windowRange({ ...base, count: 20, scrollTop: 25000 });
    expect(r.last).toBe(20);
    expect(r.first).toBe(10);
  });

  it("renders a bounded number of rows however long the list is (50,000 books)", () => {
    for (const scrollTop of [0, 1_234_567, 750_000, 1_499_700]) {
      const r = windowRange({ count: 50_000, rowHeight: 30, viewportHeight: 900, scrollTop });
      expect(r.end - r.start).toBeLessThan(100);
      expect(r.padTop + (r.end - r.start) * 30 + r.padBottom).toBe(1_500_000);
    }
  });
});

describe("scrollTopForIndex", () => {
  const v = { count: 100, rowHeight: 30, viewportHeight: 300 };

  it("leaves the scroll alone when the row is already visible", () => {
    expect(scrollTopForIndex(5, { ...v, scrollTop: 0 })).toBe(0);
  });

  it("scrolls up just far enough to show a row above", () => {
    expect(scrollTopForIndex(2, { ...v, scrollTop: 300 })).toBe(60);
  });

  it("scrolls down just far enough to show a row below", () => {
    expect(scrollTopForIndex(20, { ...v, scrollTop: 0 })).toBe(21 * 30 - 300);
  });

  it("clamps the index and the result to the list", () => {
    expect(scrollTopForIndex(500, { ...v, scrollTop: 0 })).toBe(100 * 30 - 300);
    expect(scrollTopForIndex(-5, { ...v, scrollTop: 90 })).toBe(0);
    expect(scrollTopForIndex(3, { ...v, count: 0, scrollTop: 90 })).toBe(0);
  });
});

describe("navigateIndex", () => {
  it("moves by one, a page, and to the ends, clamped", () => {
    expect(navigateIndex("ArrowDown", 5, 100, 10)).toBe(6);
    expect(navigateIndex("ArrowUp", 0, 100, 10)).toBe(0);
    expect(navigateIndex("PageDown", 5, 100, 10)).toBe(14);
    expect(navigateIndex("PageUp", 5, 100, 10)).toBe(0);
    expect(navigateIndex("PageDown", 95, 100, 10)).toBe(99);
    expect(navigateIndex("Home", 50, 100, 10)).toBe(0);
    expect(navigateIndex("End", 50, 100, 10)).toBe(99);
  });

  it("starts at the first row when nothing is selected, and ignores other keys", () => {
    expect(navigateIndex("ArrowDown", -1, 100, 10)).toBe(0);
    expect(navigateIndex("a", 3, 100, 10)).toBeNull();
    expect(navigateIndex("ArrowDown", -1, 0, 10)).toBeNull();
  });
});

describe("grid", () => {
  it("fits as many cards per row as the width allows", () => {
    expect(gridMetrics(1000, 100).columns).toBe(7); // (1000+16)/(120+16)
    expect(gridMetrics(120, 100).columns).toBe(1);
    expect(gridMetrics(0, 100).columns).toBe(1);
  });

  it("stretches cards to fill the row and sizes rows from the card width", () => {
    const m = gridMetrics(1000, 100);
    expect(m.cardWidth * m.columns + 16 * (m.columns - 1)).toBeCloseTo(1000);
    expect(m.rowHeight).toBe(Math.round(m.cardWidth * 1.5 + 48 + 16));
    expect(m.rows).toBe(Math.ceil(100 / 7));
  });

  it("re-flows on resize", () => {
    expect(gridMetrics(500, 100).columns).toBeLessThan(gridMetrics(1500, 100).columns);
    expect(gridMetrics(500, 100).rows).toBeGreaterThan(gridMetrics(1500, 100).rows);
  });

  it("maps grid rows to book indexes, clamped", () => {
    expect(gridRowItems(2, 4, 5, 100)).toEqual({ from: 10, to: 20 });
    expect(gridRowItems(18, 25, 5, 93)).toEqual({ from: 90, to: 93 });
    expect(gridRowItems(30, 40, 5, 93)).toEqual({ from: 93, to: 93 });
    expect(gridRowOf(23, 5)).toBe(4);
  });

  it("virtualises 50,000 cards into a bounded number of rendered cards", () => {
    const m = gridMetrics(1200, 50_000);
    const w = windowRange({ count: m.rows, rowHeight: m.rowHeight, scrollTop: m.rows * m.rowHeight * 0.4, viewportHeight: 800, overscan: 2 });
    const { from, to } = gridRowItems(w.start, w.end, m.columns, 50_000);
    expect(to - from).toBeLessThan(100);
  });
});
