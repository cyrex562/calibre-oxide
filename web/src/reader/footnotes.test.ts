import { describe, expect, it } from "vitest";

import { findNodeById, isNoteBody, isNoteReference, noteTextFor } from "./footnotes";
import type { SerializedNode } from "./types";

function el(n: string, opts: { x?: string; l?: string; a?: [string, string][]; c?: SerializedNode[] } = {}): SerializedNode {
  return { n, ...opts };
}

/**
 * The exact serialized shape this project's own stack produces for a
 * standard EPUB 3 footnote -- captured from a live `calibre_srv`
 * serving a real EPUB, not invented.
 */
const REAL_TREE: SerializedNode = el("body", {
  c: [
    el("p", {
      x: "Main text with a note",
      c: [
        el("a", {
          x: "1",
          l: ".",
          a: [
            ["epub:type", "noteref"],
            ["role", "doc-noteref"],
            ["href", "javascript:void(0)"],
            ["id", "ref1"],
          ],
        }),
      ],
    }),
    el("aside", {
      a: [
        ["epub:type", "footnote"],
        ["role", "doc-footnote"],
        ["id", "fn1"],
      ],
      c: [el("p", { x: "The footnote body text." })],
    }),
  ],
});

describe("isNoteReference", () => {
  // Verified against a real EPUB: `epub:type` survives serialization
  // verbatim. Reading unserialize.ts's prefix-collapsing note the
  // other way -- looking for a bare `type` -- would find nothing.
  it("recognises the EPUB 3 vocabulary", () => {
    expect(isNoteReference("noteref", null)).toBe(true);
  });

  it("recognises the ARIA DPUB role", () => {
    expect(isNoteReference(null, "doc-noteref")).toBe(true);
  });

  it("recognises a value among several tokens", () => {
    expect(isNoteReference("pagebreak noteref", null)).toBe(true);
  });

  // A substring test would match this; these are token lists.
  it("does not match a token that merely contains the word", () => {
    expect(isNoteReference("not-a-noteref", null)).toBe(false);
    expect(isNoteReference(null, "doc-noterefs")).toBe(false);
  });

  it("is false for an ordinary link", () => {
    expect(isNoteReference(null, null)).toBe(false);
    expect(isNoteReference("", "")).toBe(false);
    expect(isNoteReference("chapter", "link")).toBe(false);
  });
});

describe("isNoteBody", () => {
  it("accepts the several names real books use", () => {
    for (const v of ["footnote", "endnote", "rearnote", "note"]) {
      expect(isNoteBody(v, null), v).toBe(true);
    }
    expect(isNoteBody(null, "doc-endnote")).toBe(true);
  });

  it("is false for ordinary content", () => {
    expect(isNoteBody("chapter", "main")).toBe(false);
  });
});

describe("findNodeById", () => {
  it("finds a node nested anywhere", () => {
    expect(findNodeById(REAL_TREE, "fn1")).not.toBeNull();
    expect(findNodeById(REAL_TREE, "ref1")).not.toBeNull();
  });

  it("returns null for an id that is not there", () => {
    expect(findNodeById(REAL_TREE, "nope")).toBeNull();
  });
});

describe("noteTextFor", () => {
  it("extracts the note body", () => {
    expect(noteTextFor(REAL_TREE, "fn1")).toBe("The footnote body text.");
  });

  it("returns null when the id is missing, so the caller can fall back to navigating", () => {
    expect(noteTextFor(REAL_TREE, "nope")).toBeNull();
  });

  // The root's tail is whatever followed the note in the document --
  // including it would drag the next paragraph into the popup.
  it("does not swallow the text that follows the note", () => {
    const tree = el("body", {
      c: [el("aside", { a: [["id", "fn1"]], l: "This belongs to the document, not the note.", c: [el("p", { x: "Note body." })] })],
    });
    expect(noteTextFor(tree, "fn1")).toBe("Note body.");
  });

  describe("back-links", () => {
    // Almost every real footnote ends with one. It is navigation
    // furniture, and inside a popup dismissed by clicking away it is
    // pure noise.
    it("drops an explicitly marked back-link", () => {
      const tree = el("aside", {
        a: [["id", "fn1"]],
        c: [el("p", { x: "The note." }), el("a", { x: "return to text", a: [["epub:type", "backlink"]] })],
      });
      expect(noteTextFor(tree, "fn1")).toBe("The note.");
    });

    it("drops an unmarked arrow back-link", () => {
      const tree = el("aside", { a: [["id", "fn1"]], c: [el("p", { x: "The note." }), el("a", { x: "↩" })] });
      expect(noteTextFor(tree, "fn1")).toBe("The note.");
    });

    it("keeps a real link that is not a back-link", () => {
      const tree = el("aside", { a: [["id", "fn1"]], c: [el("p", { x: "See" }), el("a", { x: "Smith 1998" })] });
      expect(noteTextFor(tree, "fn1")).toContain("Smith 1998");
    });
  });

  it("collapses whitespace so a popup reads as one paragraph", () => {
    const tree = el("aside", { a: [["id", "fn1"]], c: [el("p", { x: "Line one.\n\n   " }), el("p", { x: "Line two." })] });
    expect(noteTextFor(tree, "fn1")).toBe("Line one. Line two.");
  });
});
