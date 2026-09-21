import { describe, expect, it } from "vitest";

import { clampWidth, columnsFor, DEFAULT_TABLE_PREFS, DEFAULT_VISIBLE_KEYS, formatCell, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, nextSort, resolveColumns, type BookColumn, type TablePrefs } from "./columns";
import type { BookSummary, FieldMetaEntry } from "./types";

/**
 * A book row shaped exactly like a real one from `/ajax/books` --
 * captured from a live `calibre_srv` against a real library rather
 * than invented, since the point of most of these tests is that the
 * formatter matches what the server actually sends.
 */
function book(overrides: Partial<BookSummary> = {}): BookSummary {
  return {
    id: 1,
    title: "Deep Work",
    authors: ["Cal Newport"],
    author_sort: "Newport, Cal",
    series: null,
    series_index: 1.0,
    rating: null,
    tags: [],
    pubdate: "2026-09-21T12:20:34.407006180+00:00",
    timestamp: "2026-09-21 12:20:34",
    last_modified: "2000-01-01 00:00:00+00:00",
    cover: "/get/cover/1",
    thumbnail: "/get/thumb/1",
    formats: ["txt"],
    main_format: { txt: "/get/txt/1" },
    other_formats: {},
    publisher: null,
    languages: [],
    identifiers: "",
    isbn: "",
    comments: null,
    uuid: "dc6782ed-8ab8-4020-8e6b-6c50884114a8",
    ...overrides,
  } as BookSummary;
}

function col(key: string, kind: BookColumn["kind"]): BookColumn {
  return { key, label: key, kind, width: 100 };
}

describe("formatCell", () => {
  it("joins authors with an ampersand and tags with commas", () => {
    // Two different separators on purpose: "A & B" is how a book's
    // authorship reads, "a, b" is how a tag set reads.
    expect(formatCell(book({ authors: ["Ann Lee", "Bo Fox"] }), col("authors", "list"))).toBe("Ann Lee & Bo Fox");
    expect(formatCell(book({ tags: ["focus", "work"] }), col("tags", "list"))).toBe("focus, work");
  });

  it("renders an empty cell for every flavour of absent", () => {
    // Real rows use all three for "nothing here", and `String(null)`
    // would put the word "null" in the table.
    expect(formatCell(book({ publisher: null }), col("publisher", "text"))).toBe("");
    expect(formatCell(book({ series: undefined }), col("series", "text"))).toBe("");
    expect(formatCell(book({ tags: [] }), col("tags", "list"))).toBe("");
  });

  it("drops empty entries inside a list rather than leaving stray separators", () => {
    expect(formatCell(book({ tags: ["a", "", "b"] }), col("tags", "list"))).toBe("a, b");
  });

  it("upper-cases formats", () => {
    // The row stores them lowercase; every other surface shows them up.
    expect(formatCell(book({ formats: ["txt", "epub"] }), col("formats", "formats"))).toBe("TXT, EPUB");
  });

  it("blanks calibre's year-101 undefined date instead of printing it", () => {
    // Upstream's UNDEFINED_DATE. Without this a column of real dates
    // gets "1/1/101" scattered through it, which reads as corruption.
    expect(formatCell(book({ pubdate: "0101-01-01T00:00:00+00:00" }), col("pubdate", "date"))).toBe("");
  });

  it("blanks an unparseable date rather than showing Invalid Date", () => {
    expect(formatCell(book({ pubdate: "not a date" }), col("pubdate", "date"))).toBe("");
  });

  it("formats a real date", () => {
    expect(formatCell(book({ timestamp: "2026-09-21 12:20:34" }), col("timestamp", "date"))).not.toBe("");
  });

  it("renders ratings as stars, with a half star for the halves", () => {
    // `ajax::book_json` has already halved calibre's 0..10 scale.
    expect(formatCell(book({ rating: 4 }), col("rating", "rating"))).toBe("★★★★");
    expect(formatCell(book({ rating: 3.5 }), col("rating", "rating"))).toBe("★★★½");
    expect(formatCell(book({ rating: 0 }), col("rating", "rating"))).toBe("");
    expect(formatCell(book({ rating: null }), col("rating", "rating"))).toBe("");
  });

  it("never dumps a raw object into a cell", () => {
    // `main_format` is an object; if it ever reached a text column,
    // "[object Object]" is the worst possible thing to show.
    expect(formatCell(book(), col("main_format", "text"))).toBe("");
  });
});

describe("columnsFor", () => {
  const sortable: [string, string][] = [
    ["title", "Title"],
    ["authors", "Authors"],
    ["size", "Size"],
    ["ondevice", "On device"],
    ["cover", "Cover"],
  ];

  it("marks sortable columns and leaves the rest unsortable", () => {
    const columns = columnsFor({}, sortable);
    expect(columns.find((c) => c.key === "title")?.sortKey).toBe("title");
    expect(columns.find((c) => c.key === "comments")?.sortKey).toBeUndefined();
  });

  // The trap this function exists for. `sortable_fields` contains
  // `size`, `ondevice` and `cover`, none of which appear in a book
  // row -- deriving columns from that list gives three columns that
  // can never show anything.
  it("does not invent columns for sortable fields that are not in a book row", () => {
    const keys = columnsFor({}, sortable).map((c) => c.key);
    expect(keys).not.toContain("size");
    expect(keys).not.toContain("ondevice");
    expect(keys).not.toContain("cover");
  });

  it("includes custom columns under the bare key a book row uses", () => {
    const fm: Record<string, FieldMetaEntry> = {
      "#read": { key: "#read", label: "read", name: "Read?", datatype: "bool", is_custom: true, is_editable: true },
    };
    const custom = columnsFor(fm, [["#read", "Read?"]]).find((c) => c.key === "read");
    expect(custom).toBeDefined();
    expect(custom?.label).toBe("Read?");
    // Sorting goes through the `#`-prefixed key, display through the bare one.
    expect(custom?.sortKey).toBe("#read");
  });

  it("ignores non-custom entries in the field metadata", () => {
    const fm: Record<string, FieldMetaEntry> = {
      title: { key: "title", label: "title", name: "Title", datatype: "text", is_custom: false, is_editable: true },
    };
    expect(columnsFor(fm, []).filter((c) => c.key === "title")).toHaveLength(1);
  });
});

describe("resolveColumns", () => {
  const available = columnsFor({}, [["title", "Title"]]);

  it("returns the user's columns in the user's order", () => {
    const prefs: TablePrefs = { view: "table", columns: ["tags", "title"], widths: {} };
    expect(resolveColumns(available, prefs).map((c) => c.key)).toEqual(["tags", "title"]);
  });

  it("drops columns the library no longer has", () => {
    // A deleted custom column would otherwise render forever empty.
    const prefs: TablePrefs = { view: "table", columns: ["title", "#gone", "tags"], widths: {} };
    expect(resolveColumns(available, prefs).map((c) => c.key)).toEqual(["title", "tags"]);
  });

  // Without this, a user who hides every column has a table with no
  // columns and no way back short of editing stored prefs by hand.
  it("falls back to the defaults rather than rendering an empty table", () => {
    const prefs: TablePrefs = { view: "table", columns: [], widths: {} };
    expect(resolveColumns(available, prefs).map((c) => c.key)).toEqual(DEFAULT_VISIBLE_KEYS);
  });

  it("applies saved widths and clamps absurd ones", () => {
    const prefs: TablePrefs = { view: "table", columns: ["title", "tags"], widths: { title: 500, tags: 5 } };
    const resolved = resolveColumns(available, prefs);
    expect(resolved[0].width).toBe(500);
    expect(resolved[1].width).toBe(MIN_COLUMN_WIDTH);
  });

  it("every default column actually exists", () => {
    // Guards a typo in DEFAULT_VISIBLE_KEYS, which would silently
    // shrink the first-run table.
    const keys = available.map((c) => c.key);
    for (const k of DEFAULT_VISIBLE_KEYS) expect(keys, `${k} is not a real column`).toContain(k);
  });
});

describe("clampWidth", () => {
  it("keeps columns within usable bounds", () => {
    expect(clampWidth(10)).toBe(MIN_COLUMN_WIDTH);
    expect(clampWidth(10_000)).toBe(MAX_COLUMN_WIDTH);
    expect(clampWidth(200)).toBe(200);
  });

  it("survives a corrupt stored width", () => {
    expect(clampWidth(Number.NaN)).toBe(MIN_COLUMN_WIDTH);
  });
});

describe("nextSort", () => {
  const title: BookColumn = { key: "title", label: "Title", kind: "text", width: 100, sortKey: "title" };
  const added: BookColumn = { key: "timestamp", label: "Date added", kind: "date", width: 100, sortKey: "timestamp" };
  const comments: BookColumn = { key: "comments", label: "Comments", kind: "text", width: 100 };

  it("flips direction when the column is already the primary sort", () => {
    expect(nextSort(title, "title", "asc")).toEqual({ sort: "title", order: "desc" });
    expect(nextSort(title, "title", "desc")).toEqual({ sort: "title", order: "asc" });
  });

  it("starts a text column ascending", () => {
    expect(nextSort(title, "timestamp", "desc")).toEqual({ sort: "title", order: "asc" });
  });

  // Nobody clicking "Date added" wants the oldest book first.
  it("starts a date column newest-first", () => {
    expect(nextSort(added, "title", "asc")).toEqual({ sort: "timestamp", order: "desc" });
  });

  it("refuses to sort by an unsortable column", () => {
    expect(nextSort(comments, "title", "asc")).toBeNull();
  });

  it("compares against the primary field of a multi-field sort", () => {
    // `sort` is a comma-joined list; only the first field decides
    // whether a click is a flip or a new sort.
    expect(nextSort(title, "title,authors", "asc")).toEqual({ sort: "title,authors", order: "desc" });
    expect(nextSort(added, "title,authors", "asc")).toEqual({ sort: "timestamp", order: "desc" });
  });
});

describe("defaults", () => {
  it("opens in the table view", () => {
    // The deliberate choice for a PDF-first library, where covers
    // carry almost no information.
    expect(DEFAULT_TABLE_PREFS.view).toBe("table");
  });
});
