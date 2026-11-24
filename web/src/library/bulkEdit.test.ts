import { describe, expect, it } from "vitest";

import { applyListEdit, changesFor, describeBulkResult, uniformChanges, InvalidPatternError, inverseChanges, isEmptySpec, parseList, replaceIn, validateSpec, type BulkEditSpec } from "./bulkEdit";
import type { BookSummary } from "./types";

function book(overrides: Partial<BookSummary> = {}): BookSummary {
  return {
    id: 1,
    title: "Deep Work",
    authors: ["Cal Newport"],
    series: null,
    series_index: 1,
    rating: null,
    tags: ["focus", "SciFi"],
    publisher: "Acme",
    languages: ["eng"],
    comments: null,
    pubdate: null,
    timestamp: null,
    cover: "",
    thumbnail: "",
    formats: ["pdf"],
    main_format: null,
    other_formats: {},
    ...overrides,
  } as BookSummary;
}

const EMPTY: BulkEditSpec = { seriesIndex: null, rating: null };

describe("parseList", () => {
  it("trims and drops empties", () => {
    expect(parseList(" a , ,b ,, c ")).toEqual(["a", "b", "c"]);
  });
});

describe("applyListEdit", () => {
  it("adds without duplicating what is already there", () => {
    // Re-adding an existing tag must be a no-op, not a second copy.
    expect(applyListEdit(["focus"], { mode: "add", value: "focus, work" })).toEqual(["focus", "work"]);
  });

  it("collapses duplicates within the input itself", () => {
    expect(applyListEdit([], { mode: "add", value: "a, A, a" })).toEqual(["a"]);
  });

  // A user typing "scifi" to remove "SciFi" means that tag, and
  // nothing in the UI would explain why it had not worked.
  it("removes case-insensitively", () => {
    expect(applyListEdit(["focus", "SciFi"], { mode: "remove", value: "scifi" })).toEqual(["focus"]);
  });

  it("preserves the original casing of values it keeps", () => {
    expect(applyListEdit(["SciFi"], { mode: "add", value: "scifi" })).toEqual(["SciFi"]);
  });

  it("replaces wholesale, including with nothing", () => {
    expect(applyListEdit(["a", "b"], { mode: "replace", value: "c" })).toEqual(["c"]);
    // "Replace with empty" is a real way to clear a field.
    expect(applyListEdit(["a"], { mode: "replace", value: "" })).toEqual([]);
  });

  it("leaves the list alone when add/remove are given nothing", () => {
    expect(applyListEdit(["a"], { mode: "add", value: "  " })).toEqual(["a"]);
    expect(applyListEdit(["a"], { mode: "remove", value: "" })).toEqual(["a"]);
  });
});

describe("replaceIn", () => {
  it("replaces every occurrence in plain mode", () => {
    expect(replaceIn("a-b-c", { find: "-", replace: "_", useRegex: false })).toBe("a_b_c");
  });

  // The reason plain mode exists: these are regex metacharacters, and
  // a user searching for them means them literally.
  it("treats regex metacharacters literally in plain mode", () => {
    expect(replaceIn("C++ primer", { find: "C++", replace: "C", useRegex: false })).toBe("C primer");
    expect(replaceIn("Title (draft)", { find: "(draft)", replace: "", useRegex: false })).toBe("Title ");
  });

  it("replaces every occurrence in regex mode", () => {
    expect(replaceIn("a1b2c3", { find: "[0-9]", replace: "", useRegex: true })).toBe("abc");
  });

  it("supports capture groups", () => {
    expect(replaceIn("Newport, Cal", { find: "(\\w+), (\\w+)", replace: "$2 $1", useRegex: true })).toBe("Cal Newport");
  });

  it("raises a typed error for a pattern that does not compile", () => {
    expect(() => replaceIn("x", { find: "(unclosed", replace: "", useRegex: true })).toThrow(InvalidPatternError);
  });

  it("is a no-op when nothing was entered", () => {
    expect(replaceIn("unchanged", { find: "", replace: "x", useRegex: false })).toBe("unchanged");
  });
});

describe("changesFor", () => {
  // The point of returning null: a bulk edit over hundreds of books
  // should not POST for every one, and every needless write bumps
  // last_modified.
  it("returns null when the book already matches", () => {
    expect(changesFor(book(), { ...EMPTY, tags: { mode: "add", value: "focus" } })).toBeNull();
    expect(changesFor(book(), EMPTY)).toBeNull();
  });

  it("writes only the fields that actually changed", () => {
    const changes = changesFor(book(), { ...EMPTY, publisher: "Penguin", tags: { mode: "add", value: "focus" } });
    expect(changes).toEqual({ publisher: "Penguin" });
  });

  it("covers the fields the old bulk edit could not reach", () => {
    const changes = changesFor(book(), {
      ...EMPTY,
      authors: { mode: "replace", value: "Ann Lee, Bo Fox" },
      languages: { mode: "add", value: "fra" },
      publisher: "Penguin",
      pubdate: "2020-01-01",
      comments: "A note",
      seriesIndex: 3,
    });
    expect(changes).toEqual({
      authors: ["Ann Lee", "Bo Fox"],
      languages: ["eng", "fra"],
      publisher: "Penguin",
      pubdate: "2020-01-01",
      comments: "A note",
      series_index: 3,
    });
  });

  it("passes custom column values straight through", () => {
    expect(changesFor(book(), { ...EMPTY, custom: { shelf: "A3" } })).toEqual({ shelf: "A3" });
  });

  it("ignores blank custom column values rather than clearing the field", () => {
    expect(changesFor(book(), { ...EMPTY, custom: { shelf: "" } })).toBeNull();
  });

  it("treats a zero rating as a real value, not as absent", () => {
    // `if (rating)` would silently drop a deliberate "clear the rating".
    expect(changesFor(book({ rating: 4 }), { ...EMPTY, rating: 0 })).toEqual({ rating: 0 });
  });

  it("treats series index 0 as a real value too", () => {
    expect(changesFor(book(), { ...EMPTY, seriesIndex: 0 })).toEqual({ series_index: 0 });
  });

  describe("search and replace", () => {
    it("rewrites a single-valued field", () => {
      const changes = changesFor(book(), { ...EMPTY, searchReplace: { field: "title", find: "Deep", replace: "Shallow", useRegex: false } });
      expect(changes).toEqual({ title: "Shallow Work" });
    });

    // Multi-valued fields are joined for the replace, so the result
    // must be split back apart -- otherwise the whole author list
    // collapses into one name containing commas.
    it("keeps a multi-valued field multi-valued", () => {
      const b = book({ authors: ["Ann Lee", "Bo Lee"] });
      const changes = changesFor(b, { ...EMPTY, searchReplace: { field: "authors", find: "Lee", replace: "Fox", useRegex: false } });
      expect(changes).toEqual({ authors: ["Ann Fox", "Bo Fox"] });
    });

    it("returns null when the pattern matches nothing", () => {
      expect(changesFor(book(), { ...EMPTY, searchReplace: { field: "title", find: "zzz", replace: "x", useRegex: false } })).toBeNull();
    });

    // A replace should compose with the other edits, not fight them
    // over the same field.
    it("runs against the value the other edits produced", () => {
      const changes = changesFor(book(), {
        ...EMPTY,
        publisher: "Penguin Books",
        searchReplace: { field: "publisher", find: " Books", replace: "", useRegex: false },
      });
      expect(changes).toEqual({ publisher: "Penguin" });
    });

    it("handles a field that is currently empty", () => {
      expect(changesFor(book({ publisher: null }), { ...EMPTY, searchReplace: { field: "publisher", find: "x", replace: "y", useRegex: false } })).toBeNull();
    });
  });
});

describe("validateSpec", () => {
  it("catches a bad regex before any book is touched", () => {
    const msg = validateSpec({ ...EMPTY, searchReplace: { field: "title", find: "(unclosed", replace: "", useRegex: true } });
    expect(msg).toMatch(/Invalid search pattern/);
  });

  it("does not police plain-text searches", () => {
    expect(validateSpec({ ...EMPTY, searchReplace: { field: "title", find: "(unclosed", replace: "", useRegex: false } })).toBeNull();
  });

  it("accepts an empty spec", () => {
    expect(validateSpec(EMPTY)).toBeNull();
  });
});

describe("isEmptySpec", () => {
  it("recognises a spec that would do nothing", () => {
    expect(isEmptySpec(EMPTY)).toBe(true);
    expect(isEmptySpec({ ...EMPTY, tags: { mode: "add", value: "   " } })).toBe(true);
    expect(isEmptySpec({ ...EMPTY, custom: { shelf: "" } })).toBe(true);
  });

  it("recognises a spec that would", () => {
    expect(isEmptySpec({ ...EMPTY, publisher: "x" })).toBe(false);
    expect(isEmptySpec({ ...EMPTY, rating: 0 })).toBe(false);
    // "Replace with nothing" is a real edit: it clears the field.
    expect(isEmptySpec({ ...EMPTY, tags: { mode: "replace", value: "" } })).toBe(false);
  });
});

describe("inverseChanges", () => {
  it("holds the old value of exactly the fields written", () => {
    const b = book({ series: "Dune", series_index: 2, rating: 4, comments: "old" });
    const inverse = inverseChanges(b, { tags: ["x"], publisher: "New", series: "Other", series_index: 9, rating: 1, comments: "new" });
    expect(inverse.changes).toEqual({ tags: ["focus", "SciFi"], publisher: "Acme", series: "Dune", series_index: 2, rating: 4, comments: "old" });
    expect(inverse.lost).toEqual([]);
  });

  it("writes back empty for a field that had no value, where empty is a value the server takes", () => {
    const inverse = inverseChanges(book({ series: null, series_index: null, rating: null, comments: null, publisher: undefined }), { series: "S", series_index: 3, rating: 5, comments: "c", publisher: "P" });
    expect(inverse.changes).toEqual({ series: "", series_index: 1, rating: 0, comments: "", publisher: "" });
  });

  it("names the fields that cannot be unset instead of inventing a value", () => {
    const inverse = inverseChanges(book({ pubdate: null, authors: [] }), { pubdate: "2020-01-01", authors: ["A"], shelf: "x" });
    expect(inverse.changes).toEqual({});
    expect(inverse.lost).toEqual(["pubdate", "authors", "shelf"]);
  });

  it("puts an old date back, but not the placeholder 'undefined' date", () => {
    expect(inverseChanges(book({ pubdate: "2019-05-04T00:00:00+00:00" }), { pubdate: "2020-01-01" }).changes).toEqual({ pubdate: "2019-05-04" });
    expect(inverseChanges(book({ pubdate: "0101-01-01T00:00:00+00:00" }), { pubdate: "2020-01-01" }).lost).toEqual(["pubdate"]);
  });
});

describe("describeBulkResult", () => {
  const spec = (over: Partial<BulkEditSpec>): BulkEditSpec => ({ ...EMPTY, ...over });
  it("names a single added or removed tag", () => {
    expect(describeBulkResult(spec({ tags: { mode: "add", value: "travel" } }), 5)).toBe("Added tag “travel” to 5 books");
    expect(describeBulkResult(spec({ tags: { mode: "remove", value: "a, b" } }), 1)).toBe("Removed 2 tags from 1 book");
  });
  it("lists the fields otherwise", () => {
    expect(describeBulkResult(spec({ publisher: "Acme", series: "S" }), 3)).toBe("Updated publisher and series on 3 books");
    expect(describeBulkResult(spec({ tags: { mode: "replace", value: "x" } }), 2)).toBe("Updated tags on 2 books");
    expect(describeBulkResult(spec({ publisher: "A", series: "B", comments: "C" }), 2)).toBe("Updated publisher, series and comments on 2 books");
  });
});

describe("uniformChanges", () => {
  it("is the one change set of an edit that does not depend on the book", () => {
    expect(uniformChanges({ ...EMPTY, publisher: "Acme", series: "Saga" })).toEqual({ publisher: "Acme", series: "Saga" });
    expect(uniformChanges({ ...EMPTY, rating: 8 })).toEqual({ rating: 8 });
    expect(uniformChanges({ ...EMPTY, comments: "Read it", custom: { "#shelf": "A3" } })).toEqual({ comments: "Read it", "#shelf": "A3" });
  });

  it("includes a list replaced outright, even with nothing in it (that clears it)", () => {
    expect(uniformChanges({ ...EMPTY, tags: { mode: "replace", value: "a, b" } })).toEqual({ tags: ["a", "b"] });
    expect(uniformChanges({ ...EMPTY, tags: { mode: "replace", value: "" } })).toEqual({ tags: [] });
  });

  it("is null when what is written depends on what the book has", () => {
    expect(uniformChanges({ ...EMPTY, tags: { mode: "add", value: "travel" } })).toBeNull();
    expect(uniformChanges({ ...EMPTY, publisher: "Acme", authors: { mode: "remove", value: "Ann" } })).toBeNull();
    expect(uniformChanges({ ...EMPTY, searchReplace: { field: "title", find: "x", replace: "y", useRegex: false } })).toBeNull();
  });

  it("is null for an edit that writes nothing, and ignores an empty add or remove", () => {
    expect(uniformChanges(EMPTY)).toBeNull();
    expect(uniformChanges({ ...EMPTY, tags: { mode: "add", value: "" }, publisher: "Acme" })).toEqual({ publisher: "Acme" });
  });

  it("gives every book what changesFor gives a book that has none of it", () => {
    const spec: BulkEditSpec = { ...EMPTY, publisher: "Acme", series: "Saga", rating: 6, pubdate: "2020-02-03", languages: { mode: "replace", value: "eng" } };
    expect(uniformChanges(spec)).toEqual(changesFor(book({ publisher: null, series: null, languages: [] } as never), spec));
  });
});
