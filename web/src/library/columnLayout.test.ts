import { describe, expect, it } from "vitest";

import { autoFitWidth, CHECK_COLUMN_WIDTH, dropIndex, isDrag, moveKeyBy, moveKeyTo, pickerOrder, resetColumnPrefs, resizedColumnWidth, tableWidth, toggleKey } from "./columnLayout";
import { clampWidth, columnsFor, DEFAULT_TABLE_PREFS, DEFAULT_VISIBLE_KEYS, formatCell, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, resolveColumns } from "./columns";

describe("width math", () => {
  it("grows and shrinks by the pointer delta", () => {
    expect(resizedColumnWidth(200, 50)).toBe(250);
    expect(resizedColumnWidth(200, -50)).toBe(150);
  });
  it("clamps to the minimum (about 40px) and maximum", () => {
    expect(MIN_COLUMN_WIDTH).toBe(40);
    expect(resizedColumnWidth(100, -500)).toBe(40);
    expect(resizedColumnWidth(800, 5000)).toBe(MAX_COLUMN_WIDTH);
  });
  it("sizes the table as the sum of its columns, plus the check column", () => {
    const cols = [{ width: 100 }, { width: 250 }];
    expect(tableWidth(cols, false)).toBe(350);
    expect(tableWidth(cols, true)).toBe(350 + CHECK_COLUMN_WIDTH);
  });
  it("is wider than a narrow pane rather than squeezed into it", () => {
    expect(tableWidth([{ width: 320 }, { width: 200 }, { width: 160 }, { width: 180 }], false)).toBeGreaterThan(400);
  });
  it("auto-fits to the widest of the header and the cells", () => {
    const measure = (t: string) => t.length * 7;
    expect(autoFitWidth("Title", ["A", "A much longer title"], measure)).toBe(19 * 7 + 28);
    expect(autoFitWidth("A very long header", ["x"], measure)).toBe(18 * 7 + 16 + 28);
  });
  it("auto-fit respects the clamp", () => {
    expect(autoFitWidth("x", [], () => 1)).toBe(clampWidth(1 + 16 + 28));
    expect(autoFitWidth("x", ["y"], () => 5000)).toBe(MAX_COLUMN_WIDTH);
  });
  it("persists widths through resolveColumns", () => {
    const cols = columnsFor({}, []);
    const resolved = resolveColumns(cols, { ...DEFAULT_TABLE_PREFS, widths: { title: 111 } });
    expect(resolved.find((c) => c.key === "title")!.width).toBe(111);
  });
});

describe("click vs drag", () => {
  it("stays a click under a few pixels", () => {
    expect(isDrag(0, 0)).toBe(false);
    expect(isDrag(2, 2)).toBe(false);
  });
  it("becomes a drag past the threshold", () => {
    expect(isDrag(5, 0)).toBe(true);
    expect(isDrag(-4, 0)).toBe(true);
    expect(isDrag(3, 3)).toBe(true);
  });
});

describe("drop index and move", () => {
  const spans = [
    { left: 0, right: 100 },
    { left: 100, right: 200 },
    { left: 200, right: 300 },
  ];
  it("finds the insertion slot from the midpoints", () => {
    expect(dropIndex(spans, 10)).toBe(0);
    expect(dropIndex(spans, 60)).toBe(1);
    expect(dropIndex(spans, 160)).toBe(2);
    expect(dropIndex(spans, 290)).toBe(3);
    expect(dropIndex(spans, 5000)).toBe(3);
  });
  it("moves a column right and left", () => {
    expect(moveKeyTo(["a", "b", "c", "d"], "a", 3)).toEqual(["b", "c", "a", "d"]);
    expect(moveKeyTo(["a", "b", "c", "d"], "d", 1)).toEqual(["a", "d", "b", "c"]);
    expect(moveKeyTo(["a", "b", "c"], "a", 3)).toEqual(["b", "c", "a"]);
    expect(moveKeyTo(["a", "b", "c"], "c", 0)).toEqual(["c", "a", "b"]);
  });
  it("treats dropping beside itself as a no-op", () => {
    const keys = ["a", "b", "c"];
    expect(moveKeyTo(keys, "b", 1)).toEqual(keys);
    expect(moveKeyTo(keys, "b", 2)).toEqual(keys);
    expect(moveKeyTo(keys, "zzz", 0)).toEqual(keys);
  });
  it("moves by one with the picker's buttons and stops at the ends", () => {
    expect(moveKeyBy(["a", "b", "c"], "b", -1)).toEqual(["b", "a", "c"]);
    expect(moveKeyBy(["a", "b", "c"], "b", 1)).toEqual(["a", "c", "b"]);
    expect(moveKeyBy(["a", "b", "c"], "a", -1)).toEqual(["a", "b", "c"]);
    expect(moveKeyBy(["a", "b", "c"], "c", 1)).toEqual(["a", "b", "c"]);
  });
});

describe("picker", () => {
  const available = columnsFor({}, []);
  it("lists shown columns first in display order, then the hidden ones", () => {
    const order = pickerOrder(available, ["tags", "title"]).map((c) => c.key);
    expect(order.slice(0, 2)).toEqual(["tags", "title"]);
    expect(order).toHaveLength(available.length);
    expect(new Set(order).size).toBe(available.length);
  });
  it("toggles a column on at the end and off again", () => {
    expect(toggleKey(["a", "b"], "c")).toEqual(["a", "b", "c"]);
    expect(toggleKey(["a", "b"], "a")).toEqual(["b"]);
  });
  it("resets columns and widths to the default, keeping the view mode", () => {
    const reset = resetColumnPrefs({ view: "grid", columns: ["id"], widths: { id: 99 } });
    expect(reset).toEqual({ view: "grid", columns: DEFAULT_VISIBLE_KEYS, widths: {} });
  });
});

describe("series number column", () => {
  const col = columnsFor({}, []).find((c) => c.key === "series_index")!;
  it("is labelled Number with a tooltip", () => {
    expect(col.label).toBe("Number");
    expect(col.tooltip).toBe("Number in the series (volume)");
  });
  it("shows right after Series by default", () => {
    const i = DEFAULT_VISIBLE_KEYS.indexOf("series");
    expect(DEFAULT_VISIBLE_KEYS[i + 1]).toBe("series_index");
    expect(DEFAULT_TABLE_PREFS.columns).toContain("series_index");
  });
  it("formats 2 and 2.5, and is empty without a series", () => {
    expect(formatCell({ id: 1, title: "x", series: "S", series_index: 2 }, col)).toBe("2");
    expect(formatCell({ id: 1, title: "x", series: "S", series_index: 2.5 }, col)).toBe("2.5");
    expect(formatCell({ id: 1, title: "x", series: null, series_index: 1 }, col)).toBe("");
  });
});
