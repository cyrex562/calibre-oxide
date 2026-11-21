import { describe, expect, it, vi } from "vitest";
import {
  createCompareController,
  cursorForNumber,
  cursorNumber,
  FIRST,
  folderOf,
  LAST,
  MAX_BOOKS,
  MIDDLE,
  pageAt,
  pairLines,
  pairPageStatus,
  pdfLine,
  realign,
  splitAuthors,
  stemOf,
  stemProblem,
  stepCursor,
} from "./compare";
import type { CompareApi } from "./compare";
import type { CompareBook, ComparePair, CompareResult, DuplicateGroup, ResolvePlan } from "./api";

const book = (id: number, over: Partial<CompareBook> = {}): CompareBook => ({
  book_id: id,
  title: `Book ${id}`,
  authors: ["Ann Author"],
  path: `Ann Author/Book ${id}`,
  files: [{ format: "PDF", path: `Ann Author/Book ${id}/Book ${id}.pdf`, size: 1000 * id, mtime: 1_700_000_000 }],
  pdf: { pages: 100, has_text: id === 2, scanned: true, size: 1000 * id },
  ...over,
});

const pair = (a: number, b: number, over: Partial<ComparePair> = {}): ComparePair => ({
  a,
  b,
  identical_bytes: false,
  identical_formats: [],
  exempt: false,
  pdf: {
    fingerprinted: true,
    kind: "pdf_ocr_variant",
    confidence: 0.91,
    compared: 5,
    matched: 5,
    mean_distance: 3,
    pages: [
      { a: 0, b: 1, distance: 3, matched: true },
      { a: 99, b: 100, distance: 3, matched: true },
      { a: 50, b: 51, distance: 90, matched: false },
    ],
    text_jaccard: 0.93,
  },
  evidence: [],
  ...over,
});

const result = (ids: number[], over: Partial<CompareResult> = {}): CompareResult => ({
  ids,
  books: ids.map((id) => book(id)),
  pairs: ids.length >= 2 ? [pair(ids[0], ids[1])] : [],
  suggested_keeper: ids[1] ?? ids[0],
  keeper_reason: "it has a text layer",
  pdf: { books: ids.length, fingerprinted: ids.length },
  ...over,
});

const group = (id: string, books: number[], manual: string | null = null): DuplicateGroup => ({ id, reason: "manual", books: books.map((b) => book(b)), manual_group_id: manual });

const resolvePlan = (keeper: number, others: number[]): ResolvePlan =>
  ({
    dry_run: true,
    action: "keep_best_trash_rest",
    plan_fingerprint: "fp",
    groups: [{ group_id: "g1", reason: "manual", confidence: 1, action: "keep_best_trash_rest", keeper: book(keeper), keeper_reason: "your choice", others: others.map((o) => book(o)), bytes_freed: 10 }],
    totals: { groups: 1, books: others.length, bytes_freed: 10 },
    skipped: [],
    missing_group_ids: [],
    fully_restorable: true,
    note: "",
  }) as ResolvePlan;

function fakeApi(over: Partial<Record<keyof CompareApi, unknown>> = {}): CompareApi & Record<string, ReturnType<typeof vi.fn>> {
  const api = {
    fetchCompare: vi.fn(async (ids: number[]) => result(ids)),
    setFields: vi.fn(async () => ({})),
    renameFiles: vi.fn(async (ids: number[], template: string) => ({ dry_run: false, renamed: 1, results: [{ book_id: ids[0], title: "", current: "x", proposed: template, changed: true }] })),
    planMoveBooks: vi.fn(async (_ids: number[], folder: string) => ({ plan_fingerprint: "mfp", template: "", books: 1, files: 1, conflicts: 0, already_organized: 0, skipped: [], moves: [], folder })),
    runMoveBooks: vi.fn(async () => ({ run_id: "org-1", books_planned: 1, books_moved: 1, files_moved: 1, failed: [], cancelled: false })),
    undoOrganize: vi.fn(async () => ({ run_id: "org-1", books_restored: 1, files_restored: 1, skipped: [] })),
    refreshDuplicateBooks: vi.fn(async () => ({ refreshed: 2, has_scan: true, stale: false })),
    createManualGroup: vi.fn(async (ids: number[]) => ({ manual_group: { id: "mg-1", created_at: 1, members: [], book_ids: ids }, group: group("g1", ids, "mg-1"), removed: false })),
    addToManualGroup: vi.fn(async (_g: string, ids: number[]) => ({ manual_group: { id: "mg-1", created_at: 1, members: [], book_ids: [1, 2, ...ids] }, group: group("g2", [1, 2, ...ids], "mg-1"), removed: false })),
    removeFromManualGroup: vi.fn(async () => ({ manual_group: { id: "mg-1", created_at: 1, members: [], book_ids: [1, 2] }, group: group("g3", [1, 2], "mg-1"), removed: false })),
    fetchManualGroups: vi.fn(async () => ({ count: 0, groups: [] })),
    deleteManualGroup: vi.fn(async () => ({ removed: true, group_id: "mg-1" })),
    markNotDuplicates: vi.fn(async () => ({ exemption: {}, pairs: 1, dropped_group_ids: [], needs_rescan: false })),
    planResolve: vi.fn(async () => resolvePlan(2, [1])),
    runResolve: vi.fn(async () => ({ run_id: "run-1", action: "keep_best_trash_rest", groups_planned: 1, groups_done: 1, done_groups: ["g1"], books_trashed: 1, bytes_freed: 10, trashed_files: [], errors: [], cancelled: false, fully_restorable: true })),
    restoreResolveRun: vi.fn(async () => ({ run_id: "run-1", restored: [{ book_id: 1, title: "Book 1" }], failed: [], needs_rescan: true })),
    runDuplicateScan: vi.fn(async () => ({})),
    search: vi.fn(async () => ({ book_ids: [7, 8, 1] })),
    fetchBooks: vi.fn(async (ids: number[]) => ids.map((id) => ({ id, title: `Found ${id}`, authors: ["Z"] }))),
    ...over,
  };
  return api as unknown as CompareApi & Record<string, ReturnType<typeof vi.fn>>;
}

describe("the page cursor", () => {
  it("shows each file's own first, middle and last page", () => {
    expect(pageAt(FIRST, 100)).toBe(1);
    expect(pageAt(MIDDLE, 100)).toBe(50);
    expect(pageAt(MIDDLE, 7)).toBe(4);
    expect(pageAt(LAST, 100)).toBe(100);
    expect(pageAt(LAST, 12)).toBe(12);
  });

  it("keeps a page inside a shorter file", () => {
    expect(pageAt({ anchor: "start", offset: 60 }, 40)).toBe(40);
    expect(pageAt({ anchor: "end", offset: 60 }, 40)).toBe(1);
    expect(pageAt(FIRST, 0)).toBe(1);
  });

  it("steps forward and back, and stops at the ends", () => {
    expect(stepCursor(FIRST, 1, 100)).toEqual({ anchor: "start", offset: 1 });
    expect(stepCursor(FIRST, -1, 100)).toEqual(FIRST);
    expect(stepCursor({ anchor: "start", offset: 99 }, 1, 100)).toEqual({ anchor: "start", offset: 99 });
    // Counting from the end, › still goes toward the end of the file.
    expect(stepCursor({ anchor: "end", offset: 3 }, 1, 100)).toEqual({ anchor: "end", offset: 2 });
    expect(stepCursor(LAST, 1, 100)).toEqual(LAST);
    expect(stepCursor(LAST, -1, 100)).toEqual({ anchor: "end", offset: 1 });
    expect(stepCursor(MIDDLE, -1, 100)).toEqual({ anchor: "middle", offset: -1 });
    expect(pageAt(stepCursor(MIDDLE, 1, 100), 100)).toBe(51);
  });

  it("goes to page n, from the start or from the end", () => {
    expect(cursorForNumber(12, false, 100)).toEqual({ anchor: "start", offset: 11 });
    expect(cursorForNumber(3, true, 100)).toEqual({ anchor: "end", offset: 2 });
    // Page 3 from the end is a different page of a 100-page and a 98-page file.
    const c = cursorForNumber(3, true, 100);
    expect([pageAt(c, 100), pageAt(c, 98)]).toEqual([98, 96]);
    expect(cursorForNumber(0, false, 100)).toEqual(FIRST);
    expect(cursorForNumber(500, false, 100)).toEqual({ anchor: "start", offset: 99 });
    expect(cursorNumber(cursorForNumber(3, true, 100), 100)).toBe(3);
  });

  it("switches between counting from the start and the end without moving", () => {
    const c = { anchor: "start" as const, offset: 9 };
    const e = realign(c, true, 100);
    expect(pageAt(e, 100)).toBe(10);
    expect(e).toEqual({ anchor: "end", offset: 90 });
    expect(realign(e, false, 100)).toEqual(c);
  });
});

describe("the evidence of a pair", () => {
  it("highlights a page that is one of the compared sample pages", () => {
    const p = pair(1, 2);
    expect(pairPageStatus(p, 1, 2)).toBe("matched");
    expect(pairPageStatus(p, 100, 101)).toBe("matched");
    expect(pairPageStatus(p, 51, 52)).toBe("different");
    expect(pairPageStatus(p, 5, 5)).toBeNull();
    expect(pairPageStatus(p, null, 2)).toBeNull();
    expect(pairPageStatus(pair(1, 2, { pdf: null }), 1, 2)).toBeNull();
  });

  it("says what was measured, and says so when nothing was", () => {
    expect(pdfLine(pair(1, 2))).toBe("91% · same scan, with and without OCR text · 5 of 5 sample pages match · text similarity 0.93");
    expect(pdfLine(pair(1, 2, { pdf: { fingerprinted: false } }))).toContain("Not fingerprinted yet");
    expect(pdfLine(pair(1, 2, { pdf: { fingerprinted: true, kind: null, confidence: null, compared: 4, matched: 0 } }))).toBe("The pages do not match · 0 of 4 sample pages match");
    expect(pdfLine(pair(1, 2, { pdf: null }))).toBe("");
  });

  it("lists identical bytes first, then pages, then title and author", () => {
    const lines = pairLines(pair(1, 2, { identical_bytes: true, identical_formats: ["PDF"], evidence: [{ reason: "title_author_exact", confidence: 0.9, detail: "same title" }], exempt: true }));
    expect(lines[0]).toBe("Identical files (PDF)");
    expect(lines[1]).toContain("91%");
    expect(lines[2]).toContain("same title");
    expect(lines[lines.length - 1]).toContain("not duplicates");
  });
});

describe("loading", () => {
  it("reads the set, fills the drafts and suggests the server's keeper", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2, 3], { group: group("g1", [1, 2, 3]) });
    expect(api.fetchCompare).toHaveBeenCalledWith([1, 2, 3]);
    expect(c.state.result?.books).toHaveLength(3);
    expect(c.state.keeperId).toBe(2);
    expect(c.state.drafts[1]).toMatchObject({ title: "Book 1", authors: "Ann Author", stem: "Book 1", folder: "Ann Author/Book 1" });
    expect(c.state.groupId).toBe("g1");
  });

  it("refuses fewer than two or more than eight books without asking the server", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1]);
    expect(c.state.error).toContain("2 to 8");
    await c.open([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(c.state.error).toContain("2 to 8");
    expect(api.fetchCompare).not.toHaveBeenCalled();
  });

  it("reports a server error and leaves the view empty", async () => {
    const c = createCompareController(fakeApi({ fetchCompare: vi.fn(async () => Promise.reject(new Error("boom"))) }));
    await c.open([1, 2]);
    expect(c.state.error).toBe("boom");
    expect(c.state.result).toBeNull();
  });
});

describe("page sync", () => {
  async function opened() {
    const api = fakeApi({
      fetchCompare: vi.fn(async (ids: number[]) =>
        result(ids, { books: [book(1, { pdf: { pages: 100, has_text: false, scanned: true, size: 1 } }), book(2, { pdf: { pages: 98, has_text: true, scanned: true, size: 2 } }), book(3, { pdf: null, files: [{ format: "EPUB", path: "x.epub", size: 1, mtime: 1 }] })] }),
      ),
    });
    const c = createCompareController(api);
    await c.open([1, 2, 3]);
    return c;
  }

  it("moves every column together, each in its own file", async () => {
    const c = await opened();
    expect([c.pageFor(1), c.pageFor(2), c.pageFor(3)]).toEqual([1, 1, null]);
    c.last();
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([100, 98]);
    c.middle();
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([50, 49]);
    c.first();
    c.step(1);
    c.step(1);
    expect([c.pageFor(1), c.pageFor(2), c.pageNumber()]).toEqual([3, 3, 3]);
    c.step(-1);
    expect(c.pageFor(2)).toBe(2);
  });

  it("counts from the end when asked: page n from the end of each file", async () => {
    const c = await opened();
    c.goTo(5);
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([5, 5]);
    c.setAlignFromEnd(true);
    // Still on page 5 of the first file, which is page 96 from the end.
    expect(c.pageFor(1)).toBe(5);
    expect(c.pageNumber()).toBe(96);
    c.goTo(1);
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([100, 98]);
    c.goTo(3);
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([98, 96]);
    c.step(1);
    expect([c.pageFor(1), c.pageFor(2)]).toEqual([99, 97]);
    c.setAlignFromEnd(false);
    expect(c.pageFor(1)).toBe(99);
    expect(c.state.cursor.anchor).toBe("start");
  });

  it("tells the pair evidence which pages are on screen", async () => {
    const c = await opened();
    // The test pair's matched sample pages are (a 0, b 1) and (a 99, b 100).
    expect(c.pairStatus(pair(1, 2))).toBeNull();
    c.state.cursor = { anchor: "start", offset: 0 };
    const p = pair(1, 2, { pdf: { fingerprinted: true, pages: [{ a: 0, b: 0, distance: 2, matched: true }] } });
    expect(c.pairStatus(p)).toBe("matched");
  });
});

describe("editing in place", () => {
  it("saves a changed title through the set-fields api and says nothing for an unchanged one", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    expect(await c.saveTitle(1)).toBe(true);
    expect(api.setFields).not.toHaveBeenCalled();

    c.state.drafts[1].title = "  A Better Title ";
    expect(await c.saveTitle(1)).toBe(true);
    expect(api.setFields).toHaveBeenCalledWith(1, { title: "A Better Title" });
    expect(api.refreshDuplicateBooks).toHaveBeenCalledWith([1, 2]);
    expect(api.fetchCompare).toHaveBeenCalledTimes(2);
  });

  it("saves authors split on &", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[2].authors = "Ann Author & Bo Writer";
    await c.saveAuthors(2);
    expect(api.setFields).toHaveBeenCalledWith(2, { authors: ["Ann Author", "Bo Writer"] });
    expect(splitAuthors(" A &  & B ")).toEqual(["A", "B"]);
  });

  it("shows a save error against its own field and keeps what was typed", async () => {
    const api = fakeApi({ setFields: vi.fn(async (_id: number, ch: { title?: string }) => (ch.title ? Promise.reject(new Error("locked")) : {})) });
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[1].title = "New";
    c.state.drafts[1].authors = "Someone Else";
    expect(await c.saveMetadata(1)).toBe(false);
    expect(c.state.drafts[1].errors.title).toBe("locked");
    expect(c.state.drafts[1].errors.authors).toBeUndefined();
    expect(c.state.drafts[1].title).toBe("New");
    expect(api.setFields).toHaveBeenCalledWith(1, { authors: ["Someone Else"] });
  });

  it("refuses an empty title or authors box", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[1].title = "  ";
    c.state.drafts[1].authors = " & ";
    expect(await c.saveTitle(1)).toBe(false);
    expect(await c.saveAuthors(1)).toBe(false);
    expect(c.state.drafts[1].errors.title).toBeTruthy();
    expect(c.state.drafts[1].errors.authors).toBeTruthy();
    expect(api.setFields).not.toHaveBeenCalled();
  });

  it("copies this title and authors to the other columns' boxes without saving", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2, 3]);
    c.state.drafts[1].title = "The One Title";
    c.state.drafts[1].authors = "Ann & Bo";
    c.useTitleForAll(1);
    expect([c.state.drafts[2].title, c.state.drafts[3].title]).toEqual(["The One Title", "The One Title"]);
    expect(c.state.drafts[3].authors).toBe("Ann & Bo");
    expect(api.setFields).not.toHaveBeenCalled();
    await c.saveMetadata(2);
    expect(api.setFields).toHaveBeenCalledWith(2, { title: "The One Title" });
    expect(api.setFields).toHaveBeenCalledWith(2, { authors: ["Ann", "Bo"] });
  });

  it("keeps what is typed in another column when the set reloads after a save", async () => {
    const c = createCompareController(fakeApi());
    await c.open([1, 2]);
    c.state.drafts[2].title = "Half typed";
    c.state.drafts[1].title = "Saved";
    await c.saveTitle(1);
    expect(c.state.drafts[2].title).toBe("Half typed");
  });

  it("renames the file through /rename-files with the stem as a literal, and shows the name it got", async () => {
    let stem = "Book 1";
    const api = fakeApi({
      renameFiles: vi.fn(async () => {
        stem = "Clean Name (2)";
        return { dry_run: false, renamed: 1, results: [{ book_id: 1, title: "", current: "Book 1", proposed: "Clean Name (2)", changed: true }] };
      }),
      fetchCompare: vi.fn(async (ids: number[]) => result(ids, { books: ids.map((id) => book(id, id === 1 ? { files: [{ format: "PDF", path: `Ann Author/Book 1/${stem}.pdf`, size: 1, mtime: 1 }] } : {})) })),
    });
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[1].stem = "Clean Name";
    expect(await c.saveStem(1)).toBe(true);
    expect(api.renameFiles).toHaveBeenCalledWith([1], "Clean Name", false);
    expect(c.state.drafts[1].stem).toBe("Clean Name (2)");
  });

  it("refuses a stem the rename route would read as a template, and shows a rename error", async () => {
    const api = fakeApi({ renameFiles: vi.fn(async () => ({ dry_run: false, renamed: 0, results: [{ book_id: 1, title: "", current: "x", changed: false, error: "file in use" }] })) });
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[1].stem = "{title}";
    expect(await c.saveStem(1)).toBe(false);
    expect(api.renameFiles).not.toHaveBeenCalled();
    expect(stemProblem("a/b")).toBeTruthy();
    expect(stemProblem("ok name")).toBeNull();
    c.state.drafts[1].stem = "fine";
    expect(await c.saveStem(1)).toBe(false);
    expect(c.state.drafts[1].errors.stem).toBe("file in use");
  });

  it("moves a book: a dry run, then the move, and offers to undo it", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    c.state.drafts[1].folder = "Sorted/PDFs";
    expect(await c.moveBook(1)).toBe(true);
    expect(api.planMoveBooks).toHaveBeenCalledWith([1], "Sorted/PDFs");
    expect(api.runMoveBooks).toHaveBeenCalledWith([1], "Sorted/PDFs", "mfp", expect.anything());
    expect(c.state.drafts[1].move).toEqual({ runId: "org-1", from: "Ann Author/Book 1", to: "Sorted/PDFs" });
    expect(api.refreshDuplicateBooks).toHaveBeenCalled();

    expect(await c.undoMove(1)).toBe(true);
    expect(api.undoOrganize).toHaveBeenCalledWith("org-1");
    expect(c.state.drafts[1].move).toBeNull();
    expect(c.state.drafts[1].folder).toBe("Ann Author/Book 1");
  });

  it("shows a move that cannot happen against the folder box", async () => {
    const rejected = fakeApi({ planMoveBooks: vi.fn(async () => Promise.reject(new Error("the folder must stay inside the library"))) });
    const c = createCompareController(rejected);
    await c.open([1, 2]);
    c.state.drafts[1].folder = "../x";
    expect(await c.moveBook(1)).toBe(false);
    expect(c.state.drafts[1].errors.folder).toContain("inside the library");
    expect(rejected.runMoveBooks).not.toHaveBeenCalled();

    const already = fakeApi({ planMoveBooks: vi.fn(async () => ({ plan_fingerprint: "x", template: "", books: 0, files: 0, conflicts: 0, already_organized: 1, skipped: [], moves: [], folder: "f" })) });
    const c2 = createCompareController(already);
    await c2.open([1, 2]);
    c2.state.drafts[1].folder = "f";
    expect(await c2.moveBook(1)).toBe(false);
    expect(c2.state.drafts[1].errors.folder).toContain("already");
    expect(already.runMoveBooks).not.toHaveBeenCalled();

    c2.state.drafts[1].folder = "  ";
    expect(await c2.moveBook(1)).toBe(false);
  });

  it("reads names and folders from the files", () => {
    expect(stemOf("A/B/My Book.v2.pdf")).toBe("My Book.v2");
    expect(stemOf("noext")).toBe("noext");
    expect(folderOf({ path: "x/y", files: [] })).toBe("x/y");
    expect(folderOf({ files: [{ format: "PDF", path: "p/q/f.pdf", size: 1, mtime: 1 }] } as Pick<CompareBook, "path" | "files">)).toBe("p/q");
  });
});

describe("deciding", () => {
  const yes = () => true;

  it("starts with the server's suggestion and takes the person's pick", async () => {
    const c = createCompareController(fakeApi());
    await c.open([1, 2, 3]);
    expect(c.state.keeperId).toBe(2);
    c.chooseKeeper(3);
    expect(c.state.keeperId).toBe(3);
    await c.load();
    expect(c.state.keeperId).toBe(3);
    c.chooseKeeper(2);
    expect(c.state.keeperChosen).toBe(false);
  });

  it("trashes the others through resolve for exactly this group, with the chosen keeper", async () => {
    const api = fakeApi();
    const groupsChanged = vi.fn();
    const c = createCompareController(api, { groupsChanged });
    await c.open([1, 2], { group: group("g1", [1, 2], "mg-1") });
    expect(await c.keepAndTrash({ confirm: yes })).toBe(true);
    // Already exactly the group: nothing new is made.
    expect(api.createManualGroup).not.toHaveBeenCalled();
    expect(api.planResolve).toHaveBeenCalledWith({ action: "keep_best_trash_rest", groupIds: ["g1"], keepers: { g1: 2 } });
    expect(api.runResolve).toHaveBeenCalledWith({ action: "keep_best_trash_rest", groupIds: ["g1"], keepers: { g1: 2 } }, "fp", expect.anything());
    expect(c.state.decision).toMatchObject({ phase: "done", runId: "run-1", trashedIds: [1] });
    expect(c.state.bookIds).toEqual([2]);
    expect(groupsChanged).toHaveBeenCalled();
  });

  it("merges the others' metadata into the kept copy when asked", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2], { group: group("g1", [1, 2]) });
    await c.keepAndTrash({ mergeOthers: true, confirm: yes });
    expect(api.planResolve).toHaveBeenCalledWith(expect.objectContaining({ action: "merge_as_formats" }));
    expect(c.state.decision.message).toContain("Merged");
  });

  it("makes the set a group of the user's own first when it is not one", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    expect(await c.keepAndTrash({ confirm: yes })).toBe(true);
    expect(api.createManualGroup).toHaveBeenCalledWith([1, 2]);
    expect(api.planResolve).toHaveBeenCalledWith(expect.objectContaining({ groupIds: ["g1"] }));
  });

  it("scans first when there is no scan to resolve from yet", async () => {
    const api = fakeApi({
      createManualGroup: vi.fn(async (ids: number[]) => ({ manual_group: { id: "mg-1", created_at: 1, members: [], book_ids: ids }, group: null, removed: false })),
      fetchManualGroups: vi.fn(async () => ({ count: 1, groups: [{ id: "mg-1", created_at: 1, members: [], book_ids: [1, 2], snapshot_group_id: "g1" }] })),
    });
    const c = createCompareController(api);
    await c.open([1, 2]);
    expect(await c.keepAndTrash({ confirm: yes })).toBe(true);
    expect(api.runDuplicateScan).toHaveBeenCalled();
    expect(api.planResolve).toHaveBeenCalledWith(expect.objectContaining({ groupIds: ["g1"] }));
  });

  it("trashes nothing when the group is bigger than the set, and takes a group made only for this away again", async () => {
    const api = fakeApi({ createManualGroup: vi.fn(async () => ({ manual_group: { id: "mg-9", created_at: 1, members: [], book_ids: [1, 2, 5] }, group: group("g9", [1, 2, 5], "mg-9"), removed: false })) });
    const c = createCompareController(api);
    await c.open([1, 2]);
    expect(await c.keepAndTrash({ confirm: yes })).toBe(false);
    expect(api.planResolve).not.toHaveBeenCalled();
    expect(api.runResolve).not.toHaveBeenCalled();
    expect(c.state.error).toContain("larger group");
    expect(api.deleteManualGroup).toHaveBeenCalledWith("mg-9");
    expect(c.state.decision.phase).toBe("error");
  });

  it("does nothing when the person says no, and does not leave a stray group behind", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    expect(await c.keepAndTrash({ confirm: () => false })).toBe(false);
    expect(api.runResolve).not.toHaveBeenCalled();
    expect(api.deleteManualGroup).toHaveBeenCalledWith("mg-1");
    expect(c.state.decision.phase).toBe("idle");
  });

  it("does not trash when the server's plan is not for exactly these books", async () => {
    const api = fakeApi({ planResolve: vi.fn(async () => resolvePlan(2, [1, 9])) });
    const c = createCompareController(api);
    await c.open([1, 2], { group: group("g1", [1, 2]) });
    expect(await c.keepAndTrash({ confirm: yes })).toBe(false);
    expect(api.runResolve).not.toHaveBeenCalled();
    expect(c.state.error).toContain("exactly these books");
  });

  it("shows a refused resolve and undoes a finished one with the run's restore", async () => {
    const refused = fakeApi({ planResolve: vi.fn(async () => Promise.reject(new Error("409: the library has changed since the last scan"))) });
    const c0 = createCompareController(refused);
    await c0.open([1, 2], { group: group("g1", [1, 2]) });
    expect(await c0.keepAndTrash({ confirm: yes })).toBe(false);
    expect(c0.state.error).toContain("library has changed");

    const api = fakeApi();
    const libraryChanged = vi.fn();
    const c = createCompareController(api, { libraryChanged });
    await c.open([1, 2], { group: group("g1", [1, 2]) });
    await c.keepAndTrash({ confirm: yes });
    expect(await c.undoDecision()).toBe(true);
    expect(api.restoreResolveRun).toHaveBeenCalledWith("run-1");
    expect(c.state.bookIds.sort()).toEqual([1, 2]);
    expect(c.state.decision.runId).toBeNull();
    expect(libraryChanged).toHaveBeenCalled();
  });

  it("needs a keeper in the set", async () => {
    const c = createCompareController(fakeApi());
    await c.open([1, 2]);
    c.state.keeperId = 99;
    expect(await c.keepAndTrash({ confirm: yes })).toBe(false);
    expect(c.state.error).toContain("Choose");
  });

  it("marks the whole set as not duplicates", async () => {
    const api = fakeApi();
    const groupsChanged = vi.fn();
    const c = createCompareController(api, { groupsChanged });
    await c.open([1, 2, 3], { group: group("g1", [1, 2, 3], "mg-1") });
    expect(await c.notDuplicates()).toBe(true);
    expect(api.markNotDuplicates).toHaveBeenCalledWith({ bookIds: [1, 2, 3] });
    expect(c.state.manualGroupId).toBeNull();
    expect(groupsChanged).toHaveBeenCalled();
  });
});

describe("the set and the user's group", () => {
  it("searches for books to add, leaving out the ones already in the set", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    await c.searchBooks("tolkien");
    expect(api.search).toHaveBeenCalledWith(expect.objectContaining({ query: "tolkien" }));
    expect(api.fetchBooks).toHaveBeenCalledWith([7, 8]);
    expect(c.state.search.results.map((r) => r.id)).toEqual([7, 8]);
    await c.searchBooks("  ");
    expect(c.state.search.results).toEqual([]);
  });

  it("adds a book to the comparison, and to the user's group when the view is for one", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2]);
    await c.addBook(7);
    expect(c.state.bookIds).toEqual([1, 2, 7]);
    expect(api.addToManualGroup).not.toHaveBeenCalled();

    const g = fakeApi();
    const m = createCompareController(g);
    await m.open([1, 2], { group: group("g1", [1, 2], "mg-1") });
    await m.addBook(7);
    expect(g.addToManualGroup).toHaveBeenCalledWith("mg-1", [7]);
    expect(m.state.bookIds).toEqual([1, 2, 7]);
    expect(m.state.groupId).toBe("g2");
  });

  it("does not take a ninth book", async () => {
    const c = createCompareController(fakeApi());
    await c.open([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(c.state.bookIds).toHaveLength(MAX_BOOKS);
    expect(await c.addBook(9)).toBe(false);
    expect(c.state.error).toContain("8");
    expect(c.state.bookIds).toHaveLength(MAX_BOOKS);
  });

  it("removes a book from the comparison (and the user's group), but never below two", async () => {
    const api = fakeApi();
    const c = createCompareController(api);
    await c.open([1, 2, 3], { group: group("g1", [1, 2, 3], "mg-1") });
    c.chooseKeeper(3);
    expect(await c.removeBook(3)).toBe(true);
    expect(api.removeFromManualGroup).toHaveBeenCalledWith("mg-1", [3]);
    expect(c.state.bookIds).toEqual([1, 2]);
    expect(c.state.keeperId).toBe(2);
    expect(await c.removeBook(2)).toBe(false);
    expect(c.state.bookIds).toHaveLength(2);

    const plain = fakeApi();
    const p = createCompareController(plain);
    await p.open([1, 2, 3]);
    await p.removeBook(1);
    expect(plain.removeFromManualGroup).not.toHaveBeenCalled();
    expect(p.state.bookIds).toEqual([2, 3]);
  });

  it("saves the set as the user's group", async () => {
    const api = fakeApi();
    const groupsChanged = vi.fn();
    const c = createCompareController(api, { groupsChanged });
    await c.open([1, 2, 3]);
    expect(await c.saveAsGroup()).toBe(true);
    expect(api.createManualGroup).toHaveBeenCalledWith([1, 2, 3]);
    expect(c.state.manualGroupId).toBe("mg-1");
    expect(c.state.groupId).toBe("g1");
    expect(groupsChanged).toHaveBeenCalled();
  });

  it("shows the whole group when saving brought in books that were already in one", async () => {
    const api = fakeApi({ createManualGroup: vi.fn(async () => ({ manual_group: { id: "mg-1", created_at: 1, members: [], book_ids: [1, 2, 3, 4] }, group: group("g1", [1, 2, 3, 4], "mg-1"), removed: false })) });
    const c = createCompareController(api);
    await c.open([1, 2]);
    await c.saveAsGroup();
    expect(c.state.bookIds).toEqual([1, 2, 3, 4]);
    expect(c.state.note).toContain("already in your group");
  });
});
