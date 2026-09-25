import { describe, expect, it } from "vitest";

import {
  categoriesToReveal, keyOf, matchesFilter, parseExpanded, serializeExpanded,
  splitScopedFilter, toggleExpanded,
} from "./categoryTree";

describe("keyOf", () => {
  it("takes the last path segment", () => {
    expect(keyOf("/ajax/category/authors")).toBe("authors");
  });
  it("ignores a trailing slash", () => {
    expect(keyOf("/ajax/category/tags/")).toBe("tags");
  });
});

describe("expansion state", () => {
  it("round-trips", () => {
    const set = new Set(["authors", "tags"]);
    expect(parseExpanded(serializeExpanded(set))).toEqual(set);
  });

  it("collapses to nothing for malformed storage rather than throwing", () => {
    expect(parseExpanded("{not json")).toEqual(new Set());
    expect(parseExpanded('{"a":1}')).toEqual(new Set());
    expect(parseExpanded(null)).toEqual(new Set());
  });

  it("drops non-string members", () => {
    expect(parseExpanded('["authors", 3, null]')).toEqual(new Set(["authors"]));
  });

  // The whole point of the rewrite: opening one must not close another.
  it("toggles one category without disturbing the others", () => {
    const open = new Set(["authors"]);
    const next = toggleExpanded(open, "tags");
    expect(next).toEqual(new Set(["authors", "tags"]));
    expect(toggleExpanded(next, "authors")).toEqual(new Set(["tags"]));
  });

  it("does not mutate the set it is given", () => {
    const open = new Set(["authors"]);
    toggleExpanded(open, "tags");
    expect(open).toEqual(new Set(["authors"]));
  });
});

describe("matchesFilter", () => {
  it("matches any substring, case-insensitively", () => {
    expect(matchesFilter("William Gibson", "gib")).toBe(true);
    expect(matchesFilter("William Gibson", "xyz")).toBe(false);
  });

  it("matches everything when empty", () => {
    expect(matchesFilter("anything", "   ")).toBe(true);
  });

  it("takes a leading = as exact", () => {
    expect(matchesFilter("Gibson", "=gibson")).toBe(true);
    expect(matchesFilter("Gibson Jr", "=gibson")).toBe(false);
  });
});

describe("splitScopedFilter", () => {
  const known = ["authors", "tags", "series"];

  it("scopes to a named category", () => {
    expect(splitScopedFilter("tags:sf", known)).toEqual({ scope: "tags", text: "sf" });
  });

  // An author called "Smith: A Life" must not scope to a category
  // called "Smith" and silently find nothing.
  it("ignores a colon that does not name a real category", () => {
    expect(splitScopedFilter("Smith: A Life", known)).toEqual({ scope: null, text: "Smith: A Life" });
  });

  it("ignores a leading colon", () => {
    expect(splitScopedFilter(":sf", known)).toEqual({ scope: null, text: ":sf" });
  });
});

describe("categoriesToReveal", () => {
  const known = ["authors", "tags"];
  const items = {
    authors: [{ name: "William Gibson", count: 3 }],
    tags: [{ name: "cyberpunk", count: 7 }],
  };

  it("opens every category holding a match", () => {
    // "n" appears in both "William Gibson" and "cyberpunk".
    expect(categoriesToReveal("n", known, items)).toEqual(new Set(["authors", "tags"]));
  });

  it("opens only the one that matches", () => {
    expect(categoriesToReveal("gibson", known, items)).toEqual(new Set(["authors"]));
  });

  it("honours a category scope", () => {
    expect(categoriesToReveal("tags:cyber", known, items)).toEqual(new Set(["tags"]));
  });

  it("opens nothing for an empty filter", () => {
    expect(categoriesToReveal("  ", known, items)).toEqual(new Set());
  });

  it("opens nothing when a category has not been fetched yet", () => {
    expect(categoriesToReveal("gibson", known, {})).toEqual(new Set());
  });
});
