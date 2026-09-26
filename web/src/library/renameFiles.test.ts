import { describe, expect, it } from "vitest";

import { DEFAULT_RENAME_TEMPLATE, describeRename, initialRenameTemplate, previewIsCurrent, summarizeRename } from "./renameFiles";
import type { RenameFileResult } from "./types";

function result(over: Partial<RenameFileResult>): RenameFileResult {
  return { book_id: 1, title: "T", current: "T", changed: false, ...over };
}

describe("initialRenameTemplate", () => {
  // Renaming one book by hand is the common case, and the most useful
  // thing to start from is what it is currently called.
  it("offers a single book's own filename", () => {
    expect(initialRenameTemplate(["scan0001"])).toBe("scan0001");
  });

  it("falls back to a template when no literal could serve", () => {
    expect(initialRenameTemplate(["a", "b"])).toBe(DEFAULT_RENAME_TEMPLATE);
    expect(initialRenameTemplate([])).toBe(DEFAULT_RENAME_TEMPLATE);
  });

  // A book whose formats were added under different titles has no one
  // filename; the server reports that as an empty stem.
  it("falls back to a template for a book with no single filename", () => {
    expect(initialRenameTemplate([""])).toBe(DEFAULT_RENAME_TEMPLATE);
  });
});

describe("previewIsCurrent", () => {
  const key = { template: "{title}", bookIds: [1, 2] };

  it("holds while nothing has changed", () => {
    expect(previewIsCurrent(key, "{title}", [1, 2])).toBe(true);
  });

  it("does not hold before anything has been previewed", () => {
    expect(previewIsCurrent(null, "{title}", [1, 2])).toBe(false);
  });

  // The names on screen were computed from the old template, so
  // applying now would write names nobody has seen.
  it("is invalidated by editing the template", () => {
    expect(previewIsCurrent(key, "{title} - {authors}", [1, 2])).toBe(false);
  });

  it("is invalidated by changing the selection", () => {
    expect(previewIsCurrent(key, "{title}", [1])).toBe(false);
    expect(previewIsCurrent(key, "{title}", [1, 2, 3])).toBe(false);
    expect(previewIsCurrent(key, "{title}", [1, 3])).toBe(false);
  });

  // Order decides which book wins a name collision, so two books in
  // the other order can get the other names.
  it("is invalidated by reordering the selection", () => {
    expect(previewIsCurrent(key, "{title}", [2, 1])).toBe(false);
  });
});

describe("summarizeRename", () => {
  it("counts each outcome separately", () => {
    const summary = summarizeRename([
      result({ book_id: 1, changed: true, proposed: "New" }),
      result({ book_id: 2, changed: false, proposed: "T" }),
      result({ book_id: 3, changed: false, error: "the template produced an empty filename" }),
    ]);
    expect(summary).toEqual({ changed: 1, unchanged: 1, failed: 1 });
  });

  // A failure arrives with `changed: false`. Counting it as "already
  // named correctly" would tell the user the opposite of the truth.
  it("does not count a failure as already correct", () => {
    expect(summarizeRename([result({ error: "no" })])).toEqual({ changed: 0, unchanged: 0, failed: 1 });
  });

  it("handles an empty batch", () => {
    expect(summarizeRename([])).toEqual({ changed: 0, unchanged: 0, failed: 0 });
  });
});

describe("describeRename", () => {
  it("reads as a sentence before and after applying", () => {
    expect(describeRename({ changed: 3, unchanged: 0, failed: 0 }, false)).toBe("3 to rename");
    expect(describeRename({ changed: 3, unchanged: 0, failed: 0 }, true)).toBe("3 renamed");
  });

  it("mentions the other outcomes only when there are any", () => {
    expect(describeRename({ changed: 1, unchanged: 2, failed: 1 }, true)).toBe("1 renamed, 2 already named correctly, 1 could not be named");
    expect(describeRename({ changed: 1, unchanged: 0, failed: 0 }, true)).toBe("1 renamed");
  });

  // Worth saying out loud rather than showing an empty footer: a
  // template that matches every book is a real, and reassuring,
  // outcome.
  it("still says something when nothing would change", () => {
    expect(describeRename({ changed: 0, unchanged: 4, failed: 0 }, false)).toBe("0 to rename, 4 already named correctly");
  });
});
