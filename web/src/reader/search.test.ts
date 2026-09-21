import { describe, expect, it } from "vitest";

import { extractText, findMatches, totalMatches } from "./search";
import type { SerializedNode } from "./types";

/** Shorthand for building a serialized element. */
function el(n: string, opts: { x?: string; l?: string; c?: SerializedNode[] } = {}): SerializedNode {
  return { n, ...opts };
}

describe("extractText", () => {
  it("collects text and tails in document order", () => {
    const tree = el("body", { c: [el("p", { x: "First." }), el("p", { x: "Second." })] });
    expect(extractText(tree)).toBe("First. Second.");
  });

  // Both a block's opening and closing contribute a space, and the
  // source markup brings its own newlines, so without normalizing, a
  // phrase spanning a paragraph break would need exactly the right
  // run of spaces -- which nobody could guess.
  it("normalizes whitespace to single spaces", () => {
    const tree = el("body", { c: [el("p", { x: "one\n\n  two" }), el("p", { x: "three" })] });
    expect(extractText(tree)).toBe("one two three");
  });

  // Without a boundary space, "end.</p><p>Next" extracts as
  // "end.Next": a phrase search across the boundary fails and the
  // context snippet reads as gibberish.
  it("separates block elements so words do not run together", () => {
    const tree = el("body", { c: [el("p", { x: "end." }), el("p", { x: "Next" })] });
    expect(extractText(tree)).toContain("end. Next");
  });

  // Inline elements are the opposite case: a space would break the
  // word apart.
  it("does not split a word across an inline element", () => {
    const tree = el("p", { x: "un", c: [el("em", { x: "frigging", l: "believable" })] });
    expect(extractText(tree)).toBe("unfriggingbelievable");
  });

  it("ignores script and style content", () => {
    const tree = el("body", { c: [el("script", { x: "var x = 'findme';" }), el("style", { x: ".findme{}" }), el("p", { x: "real text" })] });
    const text = extractText(tree);
    expect(text).not.toContain("findme");
    expect(text).toContain("real text");
  });

  it("keeps the tail after a skipped element", () => {
    // The tail is text that followed the element in the document --
    // dropping it would lose real prose.
    const tree = el("body", { c: [el("style", { x: ".x{}", l: "after the style" }), el("p", { x: "p text" })] });
    expect(extractText(tree)).toContain("after the style");
  });

  it("keeps the tail after a comment but not the comment itself", () => {
    const tree = el("body", { c: [{ s: "c", x: "a note", l: "after the comment" } as SerializedNode] });
    const text = extractText(tree);
    expect(text).not.toContain("a note");
    expect(text).toContain("after the comment");
  });

  it("handles an empty tree", () => {
    expect(extractText(el("body"))).toBe("");
  });
});

describe("findMatches", () => {
  const text = "The quick brown fox. The quick brown dog.";

  it("finds every occurrence", () => {
    expect(findMatches(text, "quick")).toHaveLength(2);
  });

  it("is case-insensitive by default and exact on request", () => {
    expect(findMatches(text, "THE")).toHaveLength(2);
    expect(findMatches(text, "THE", { caseSensitive: true })).toHaveLength(0);
  });

  // Someone searching for "C++" means those characters. Compiling the
  // box as a regex would either throw or match something unrelated.
  it("treats regex metacharacters literally", () => {
    expect(findMatches("Learn C++ today", "C++")).toHaveLength(1);
    expect(findMatches("Published (1998) here", "(1998)")).toHaveLength(1);
    expect(findMatches("a.b and axb", "a.b")).toHaveLength(1);
  });

  it("supports whole-word matching", () => {
    expect(findMatches("cat concatenate cat.", "cat")).toHaveLength(3);
    expect(findMatches("cat concatenate cat.", "cat", { wholeWord: true })).toHaveLength(2);
  });

  // `\b` is defined over ASCII word characters, so it treats an
  // accented letter as a boundary and matches inside a word.
  it("gets word boundaries right for non-ASCII text", () => {
    expect(findMatches("il écrit", "écrit", { wholeWord: true })).toHaveLength(1);
    expect(findMatches("déjàvu", "jàv", { wholeWord: true })).toHaveLength(0);
  });

  it("returns an empty list for a blank query", () => {
    expect(findMatches(text, "")).toEqual([]);
    expect(findMatches(text, "   ")).toEqual([]);
  });

  it("reports where each match is", () => {
    const [first] = findMatches(text, "fox");
    expect(text.slice(first.index, first.index + first.text.length)).toBe("fox");
  });

  describe("context", () => {
    it("includes surrounding text with whitespace collapsed", () => {
      const messy = "Start\n\n   lots   of\tspace   around the  word  here.";
      const [m] = findMatches(messy, "word");
      expect(m.context).not.toMatch(/\s{2}/);
      expect(m.context).toContain("word");
    });

    // Collapsing whitespace shifts every offset after the first run,
    // so the offset has to be recomputed against the collapsed text
    // rather than carried over from the original.
    it("points at the match inside the collapsed context", () => {
      const messy = "aaa   bbb   ccc TARGET ddd";
      const [m] = findMatches(messy, "TARGET");
      expect(m.context.slice(m.contextOffset, m.contextOffset + m.text.length)).toBe("TARGET");
    });

    it("copes with a match at the very start", () => {
      const [m] = findMatches("TARGET trails off", "TARGET");
      expect(m.context.slice(m.contextOffset, m.contextOffset + 6)).toBe("TARGET");
    });
  });
});

describe("totalMatches", () => {
  it("sums across spine files", () => {
    const results = [
      { spineIndex: 0, name: "a.xhtml", matches: findMatches("one two one", "one") },
      { spineIndex: 1, name: "b.xhtml", matches: findMatches("one", "one") },
    ];
    expect(totalMatches(results)).toBe(3);
  });

  it("is zero for no results", () => {
    expect(totalMatches([])).toBe(0);
  });
});
