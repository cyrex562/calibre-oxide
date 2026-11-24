// What the view keeps selected and open once books are gone (C3), and what the
// right-click menu offers for two books (C8).

import { describe, expect, it } from "vitest";

import { contextMenuEntries, type LibraryActionId } from "./actions";
import type { MergeResponse } from "./api";
import { mergeOutcome } from "./merge";
import { dropRemoved } from "./useSelection";

describe("dropRemoved (after a merge or any removal)", () => {
  it("takes the removed books out of the selection and replaces the open one with the kept record", () => {
    const next = dropRemoved({ selected: new Set([302, 303, 400]), openId: 302 }, [302], 303);
    expect([...next.selected].sort()).toEqual([303, 400]);
    expect(next.openId).toBe(303);
  });
  it("closes the panel when nothing replaces the open book", () => {
    expect(dropRemoved({ selected: new Set(), openId: 5 }, [5]).openId).toBeNull();
  });
  it("leaves an open book that was not removed alone", () => {
    expect(dropRemoved({ selected: new Set([1]), openId: 9 }, [1], 2)).toEqual({ selected: new Set(), openId: 9 });
  });
});

describe("mergeOutcome", () => {
  const src = (book_id: number, deleted: boolean, error: string | null = null) => ({ book_id, deleted, error }) as unknown as MergeResponse["results"][number];
  it("names the kept record and the ones the merge deleted", () => {
    const r = { dest: 303, results: [src(302, true), src(304, false), src(305, true, "busy")] } as unknown as MergeResponse;
    expect(mergeOutcome(r)).toEqual({ dest: 303, removed: [302] });
  });
});

describe("the right-click menu with two books selected (C8)", () => {
  const handled: LibraryActionId[] = ["bulk-edit", "save-to-disk", "merge-books", "compare-duplicates", "delete-book"];
  it("offers Merge books and Compare as duplicates", () => {
    const ids = contextMenuEntries(handled, { selectionCount: 2, isDesktop: false }).map((e) => e.id);
    expect(ids).toContain("merge-books");
    expect(ids).toContain("compare-duplicates");
  });
  it("shows them greyed out for a single book", () => {
    const merge = contextMenuEntries(handled, { selectionCount: 1, isDesktop: false }).find((e) => e.id === "merge-books");
    expect(merge?.enabled ?? false).toBe(false);
  });
});
