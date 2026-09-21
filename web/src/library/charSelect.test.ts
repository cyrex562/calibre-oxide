import { describe, expect, it } from "vitest";

import { allChars, CHAR_GROUPS, insertAt, searchChars } from "./charSelect";

describe("the character list", () => {
  it("has no duplicates across groups", () => {
    const chars = allChars().map((c) => c.char);
    expect(new Set(chars).size).toBe(chars.length);
  });

  // Invisible characters are indistinguishable in a grid, so the name
  // is the only thing that tells a non-breaking space from a normal
  // one -- and inserting the wrong one is a bug nobody can see.
  it("names every character", () => {
    for (const c of allChars()) {
      expect(c.name.trim(), JSON.stringify(c.char)).not.toBe("");
    }
  });

  it("includes the invisible characters an editor actually needs", () => {
    const chars = allChars().map((c) => c.char);
    expect(chars).toContain(" "); // non-breaking space
    expect(chars).toContain("­"); // soft hyphen
    expect(chars).toContain("​"); // zero-width space
  });

  it("groups them", () => {
    expect(CHAR_GROUPS.length).toBeGreaterThan(1);
    for (const g of CHAR_GROUPS) expect(g.chars.length).toBeGreaterThan(0);
  });
});

describe("searchChars", () => {
  it("returns everything for an empty query", () => {
    expect(searchChars("")).toHaveLength(allChars().length);
    expect(searchChars("   ")).toHaveLength(allChars().length);
  });

  it("matches on name", () => {
    expect(searchChars("em dash").map((c) => c.char)).toContain("—");
  });

  it("matches on keyword", () => {
    // "nbsp" is what someone types; it is not in the name.
    expect(searchChars("nbsp").map((c) => c.char)).toContain(" ");
  });

  it("is case-insensitive", () => {
    expect(searchChars("ELLIPSIS").map((c) => c.char)).toContain("…");
  });

  // Pasting a character to find out what it is called is a real way
  // to use this.
  it("matches the character itself", () => {
    const found = searchChars("—");
    expect(found).toHaveLength(1);
    expect(found[0].name).toBe("Em dash");
  });

  it("returns nothing for a query that matches nothing", () => {
    expect(searchChars("zzzznotathing")).toEqual([]);
  });
});

describe("insertAt", () => {
  it("inserts at the cursor and moves it past the insertion", () => {
    const { text, cursor } = insertAt("ab", 1, 1, "X");
    expect(text).toBe("aXb");
    // Leaving the cursor where it was would put the next keystroke on
    // the wrong side of the character just inserted.
    expect(cursor).toBe(2);
  });

  it("replaces a selection, as every editor does", () => {
    const { text, cursor } = insertAt("hello world", 0, 5, "goodbye");
    expect(text).toBe("goodbye world");
    expect(cursor).toBe(7);
  });

  it("inserts at the start and the end", () => {
    expect(insertAt("bc", 0, 0, "a").text).toBe("abc");
    expect(insertAt("ab", 2, 2, "c").text).toBe("abc");
  });

  // A stale cursor position from a previous document would otherwise
  // slice out of range.
  it("clamps an out-of-range cursor", () => {
    expect(insertAt("ab", 99, 99, "X").text).toBe("abX");
    expect(insertAt("ab", -5, -5, "X").text).toBe("Xab");
  });

  it("copes with a reversed selection", () => {
    // `end` before `start` cannot delete backwards.
    expect(insertAt("abc", 2, 1, "X").text).toBe("abXc");
  });

  it("handles a multi-character insertion", () => {
    const { cursor } = insertAt("ab", 1, 1, "——");
    expect(cursor).toBe(3);
  });
});
