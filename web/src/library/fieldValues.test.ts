import { beforeEach, describe, expect, it, vi } from "vitest";

import { applySuggestion, clampMinChars, completionFor, completionForCustom, getFieldValues, getMinChars, invalidateFieldValues, invalidateForChanges, setMinChars, suggest, type FieldValue } from "./fieldValues";
import { columnsFor } from "./columns";
import type { FieldMetaEntry } from "./types";

const v = (name: string, count = 1): FieldValue => ({ name, count });
const names = (r: { name: string }[]) => r.map((x) => x.name);

describe("suggest ranking", () => {
  it("ranks prefix, then word-start, then contains", () => {
    const values = [v("Pinnacle Wizards", 9), v("Worlds Without Number", 1), v("Number Theory", 1), v("Renumbered", 50)];
    expect(names(suggest(values, "num"))).toEqual(["Number Theory", "Worlds Without Number", "Renumbered"]);
  });

  it("matches an acronym after prefix and word-start matches", () => {
    const values = [v("Wwn Notes", 1), v("Worlds Without Number", 5), v("Wandering Wolves Noir", 2), v("Dawwn", 9)];
    expect(names(suggest(values, "wwn"))).toEqual(["Wwn Notes", "Worlds Without Number", "Wandering Wolves Noir", "Dawwn"]);
  });

  it("orders by count within a rank", () => {
    expect(names(suggest([v("Alpha", 1), v("Alps", 7), v("Alto", 3)], "al"))).toEqual(["Alps", "Alto", "Alpha"]);
  });

  it("is case and accent insensitive, and matches by word start", () => {
    expect(names(suggest([v("Stanisław Lem"), v("Elmer")], "lem"))).toEqual(["Stanisław Lem"]); // word start
    expect(names(suggest([v("Stanisław Lem"), v("Solemn")], "lem"))).toEqual(["Stanisław Lem", "Solemn"]);
    expect(names(suggest([v("Émile Zola")], "emi"))).toEqual(["Émile Zola"]);
    expect(names(suggest([v("Eve", 1), v("Émile", 2)], "e", { minChars: 1 }))).toEqual(["Émile", "Eve"]);
    expect(names(suggest([v("émile")], "ÉM"))).toEqual(["émile"]);
  });

  it("reports the matched range for highlighting", () => {
    expect(suggest([v("Worlds Without Number")], "numb")[0].range).toEqual([15, 19]);
    expect(suggest([v("Émile Zola")], "emi")[0].range).toEqual([0, 3]);
    expect(suggest([v("Worlds Without Number")], "wwn")[0].range).toBeNull();
  });

  it("honours minChars and limit", () => {
    const values = Array.from({ length: 30 }, (_, i) => v(`Name ${i}`, i));
    expect(suggest(values, "n")).toEqual([]);
    expect(suggest(values, "n", { minChars: 1 }).length).toBe(10);
    expect(suggest(values, "na", { limit: 3 })).toHaveLength(3);
    expect(suggest(values, "nam", { minChars: 4 })).toEqual([]);
  });

  it("does not offer back exactly what is typed", () => {
    expect(names(suggest([v("Jane Doe")], "Jane Doe"))).toEqual([]);
    expect(names(suggest([v("Jane Doe")], "jane doe"))).toEqual(["Jane Doe"]);
  });
});

describe("suggest with several values", () => {
  const authors = [v("Jane Doe", 5), v("John Smith", 3), v("Jim Beam", 1)];
  it("completes only the segment after the last separator", () => {
    expect(names(suggest(authors, "Jane Doe & jo", { separator: "&" }))).toEqual(["John Smith"]);
    expect(names(suggest(authors, "ja, ji", { separator: "," }))).toEqual(["Jim Beam"]);
  });
  it("leaves out values already entered", () => {
    expect(names(suggest(authors, "John Smith & j", { separator: "&", minChars: 1 }))).toEqual(["Jane Doe", "Jim Beam"]);
    expect(names(suggest(authors, "JOHN SMITH & j", { separator: "&", minChars: 1 }))).toEqual(["Jane Doe", "Jim Beam"]);
  });
  it("does not split single-value fields", () => {
    expect(names(suggest([v("Tom & Jerry")], "tom &"))).toEqual(["Tom & Jerry"]);
  });
  it("offers nothing after a trailing separator", () => {
    expect(suggest(authors, "Jane Doe & ", { separator: "&" })).toEqual([]);
  });
});

describe("applySuggestion", () => {
  it("replaces the last segment and normalizes spacing", () => {
    expect(applySuggestion("Jane Doe&jo", "John Smith", "&")).toBe("Jane Doe & John Smith & ");
    expect(applySuggestion("a,b,  c", "Cyan", ",")).toBe("a, b, Cyan, ");
    expect(applySuggestion("jo", "John Smith", "&")).toBe("John Smith & ");
  });
  it("adds no separator for single-value fields", () => {
    expect(applySuggestion("wor", "Worlds Without Number", null)).toBe("Worlds Without Number");
  });
});

describe("completionFor", () => {
  const fm: Record<string, FieldMetaEntry> = {
    "#genre": { key: "#genre", label: "genre", name: "Genre", datatype: "text", is_custom: true, is_editable: true, is_multiple: true },
    "#mood": { key: "#mood", label: "mood", name: "Mood", datatype: "text", is_custom: true, is_editable: true },
    "#saga": { key: "#saga", label: "saga", name: "Saga", datatype: "series", is_custom: true, is_editable: true },
    "#pages": { key: "#pages", label: "pages", name: "Pages", datatype: "int", is_custom: true, is_editable: true },
  };
  const cols = columnsFor(fm, []);
  const col = (k: string) => cols.find((c) => c.key === k)!;
  it("maps standard columns", () => {
    expect(completionFor(col("authors"))).toEqual({ field: "authors", separator: "&" });
    expect(completionFor(col("tags"))).toEqual({ field: "tags", separator: "," });
    expect(completionFor(col("series"))).toEqual({ field: "series", separator: null });
    expect(completionFor(col("publisher"))).toEqual({ field: "publisher", separator: null });
    expect(completionFor(col("title"))).toBeNull();
    expect(completionFor(col("languages"))).toBeNull();
    expect(completionFor(col("rating"))).toBeNull();
  });
  it("maps custom columns", () => {
    expect(completionFor(col("genre"))).toEqual({ field: "#genre", separator: "," });
    expect(completionFor(col("mood"))).toEqual({ field: "#mood", separator: null });
    expect(completionFor(col("saga"))).toEqual({ field: "#saga", separator: null });
    expect(completionFor(col("pages"))).toBeNull();
    expect(completionForCustom(fm["#genre"])).toEqual({ field: "#genre", separator: "," });
  });
});

describe("min chars setting", () => {
  beforeEach(() => localStorage.clear());
  it("defaults to 2, clamps to 1..5 and persists", () => {
    expect(getMinChars()).toBe(2);
    expect(setMinChars(9)).toBe(5);
    expect(getMinChars()).toBe(5);
    expect(setMinChars(0)).toBe(1);
    expect(clampMinChars(NaN)).toBe(2);
  });
});

describe("value cache", () => {
  beforeEach(() => invalidateFieldValues());
  it("shares one request between concurrent callers and then the result", async () => {
    const loader = vi.fn(async () => [v("A", 2)]);
    const [a, b] = await Promise.all([getFieldValues("tags", loader), getFieldValues("tags", loader)]);
    await getFieldValues("tags", loader);
    expect(loader).toHaveBeenCalledTimes(1);
    expect(a).toBe(b);
  });
  it("refetches after invalidation, per field or all", async () => {
    const loader = vi.fn(async () => [v("A")]);
    await getFieldValues("tags", loader);
    await getFieldValues("authors", loader);
    invalidateFieldValues("tags");
    await getFieldValues("tags", loader);
    await getFieldValues("authors", loader);
    expect(loader).toHaveBeenCalledTimes(3);
    invalidateFieldValues();
    await getFieldValues("authors", loader);
    expect(loader).toHaveBeenCalledTimes(4);
  });
  it("invalidates the fields a save touched", async () => {
    const loader = vi.fn(async () => [v("A")]);
    await getFieldValues("tags", loader);
    await getFieldValues("#genre", loader);
    await getFieldValues("series", loader);
    invalidateForChanges({ tags: ["x"], genre: "y" } as never);
    await getFieldValues("tags", loader);
    await getFieldValues("#genre", loader);
    await getFieldValues("series", loader);
    expect(loader).toHaveBeenCalledTimes(5);
  });
  it("returns an empty list on failure and retries next time", async () => {
    const bad = vi.fn(async () => {
      throw new Error("down");
    });
    expect(await getFieldValues("tags", bad)).toEqual([]);
    const good = vi.fn(async () => [v("A")]);
    expect(await getFieldValues("tags", good)).toEqual([v("A")]);
  });
});

describe("suggest cost", () => {
  it("folds each known value once, not again on every keystroke", () => {
    const values = Array.from({ length: 200 }, (_, i) => v(`Authörs Number ${i}`, i));
    const spy = vi.spyOn(String.prototype, "normalize");
    try {
      suggest(values, "auth");
      const first = spy.mock.calls.length;
      suggest(values, "autho");
      suggest(values, "author");
      const more = spy.mock.calls.length - first;
      // Only the typed text is folded again: a few characters, not 200 names of 16.
      expect(first).toBeGreaterThan(values.length);
      expect(more).toBeLessThan(40);
    } finally {
      spy.mockRestore();
    }
  });
});
