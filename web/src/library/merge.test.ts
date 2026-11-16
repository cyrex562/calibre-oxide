import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultMergeDestination, describeMergeResult, describeRecord, summarizeMerge } from "./merge";
import { mergeBooks, planBulkMerge, runBulkMerge } from "./api";
import type { DuplicateBook, MergeResponse } from "./api";

const book = (book_id: number, formats: number, metadata: number, has_cover: boolean): DuplicateBook => ({ book_id, title: `Book ${book_id}`, authors: ["A"], formats, metadata, has_cover });

describe("defaultMergeDestination", () => {
  it("prefers the record with the most formats", () => {
    expect(defaultMergeDestination([book(1, 1, 8, true), book(2, 2, 0, false)])).toBe(2);
  });
  it("then the most metadata", () => {
    expect(defaultMergeDestination([book(1, 2, 1, true), book(2, 2, 3, false)])).toBe(2);
  });
  it("then a cover", () => {
    expect(defaultMergeDestination([book(1, 2, 3, false), book(2, 2, 3, true)])).toBe(2);
  });
  it("then the oldest, whatever the order they are listed in", () => {
    expect(defaultMergeDestination([book(7, 2, 3, true), book(3, 2, 3, true), book(9, 2, 3, true)])).toBe(3);
  });
  it("copes with a server that sends no summary, and with no books", () => {
    expect(defaultMergeDestination([{ book_id: 5, title: "x", authors: [] }, { book_id: 4, title: "y", authors: [] }])).toBe(4);
    expect(defaultMergeDestination([])).toBeNull();
  });
});

describe("describeRecord", () => {
  it("says why a record looks good", () => {
    expect(describeRecord(book(1, 1, 1, true))).toBe("1 format, 1 metadata field, has cover");
    expect(describeRecord(book(1, 3, 0, false))).toBe("3 formats, 0 metadata fields, no cover");
  });
});

const response = (over: Partial<MergeResponse> = {}): MergeResponse => ({
  ok: true,
  dry_run: true,
  dest: 1,
  mode: "merge_and_delete",
  results: [
    {
      book_id: 2,
      title: "Dup",
      formats_moved: ["EPUB"],
      formats_identical: ["PDF"],
      formats_discarded: ["TXT"],
      formats_kept_as_data_file: [],
      formats_left_on_source: [],
      data_files_moved: [],
      deleted: true,
      trashed: ["Dup.pdf"],
      failed: [],
      error: null,
      warnings: [],
    },
  ],
  metadata_changed: ["tags"],
  cover_from: 2,
  trashed: ["Dup.pdf"],
  failed: [],
  errors: [],
  warnings: [],
  ...over,
});

describe("summarizeMerge", () => {
  it("says what moves, what is discarded and what is trashed", () => {
    const text = summarizeMerge(response()).join("\n");
    expect(text).toContain('EPUB from "Dup"');
    expect(text).toContain("1 format already in the destination");
    expect(text).toContain('TXT of "Dup"');
    expect(text).toContain("tags");
    expect(text).toContain('cover of "Dup"');
    expect(text).toContain("1 book will be removed");
    expect(text).toContain("1 file will go to the wastebasket: Dup.pdf");
  });
  it("says nothing is deleted in keep-sources mode, and skips metadata in formats-only mode", () => {
    const keep = summarizeMerge(response({ mode: "merge_keep_sources" })).join("\n");
    expect(keep).toContain("The other books are kept.");
    expect(keep).not.toContain("wastebasket");
    const formatsOnly = summarizeMerge(response({ mode: "formats_only" })).join("\n");
    expect(formatsOnly).not.toContain("Metadata");
  });
  it("surfaces a source that cannot be merged", () => {
    const r = response();
    r.results[0].error = "EPUB: could not be verified";
    expect(summarizeMerge(r).join("\n")).toContain('Cannot merge "Dup": EPUB: could not be verified');
  });
});

describe("describeMergeResult", () => {
  it("reports trashed and un-trashable files", () => {
    const r = response({ dry_run: false, failed: [{ book_id: 2, path: "x.pdf", error: "locked" }] });
    expect(describeMergeResult(r)).toBe("Merged 1 book. 1 file sent to the wastebasket. Left in place because it could not be trashed: x.pdf.");
  });
});

afterEach(() => vi.unstubAllGlobals());

describe("the merge API client", () => {
  it("mergeBooks posts the request as JSON to the library route", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => response() });
    vi.stubGlobal("fetch", fetchMock);
    await mergeBooks({ dest: 1, sources: [2, 3], mode: "merge_keep_sources", dry_run: true });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/merge-books/default");
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body)).toEqual({ dest: 1, sources: [2, 3], mode: "merge_keep_sources", dry_run: true });
  });

  it("planBulkMerge is always a dry run", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ dry_run: true, groups: [], group_count: 0, books_to_merge: 0, plan_fingerprint: "abc" }) });
    vi.stubGlobal("fetch", fetchMock);
    const plan = await planBulkMerge();
    expect(plan.plan_fingerprint).toBe("abc");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/duplicates/merge-exact/default");
    expect(JSON.parse(init.body)).toEqual({ dry_run: true });
  });

  it("runBulkMerge sends the confirmed fingerprint, follows the job and returns its report", async () => {
    const report = { groups_planned: 2, groups_merged: 2, books_merged: 2, trashed: [], errors: [], cancelled: false };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, json: async () => ({ job_id: 9 }) })
      .mockResolvedValueOnce({ ok: true, json: async () => ({ id: 9, kind: "merge_exact", state: "running", progress: { phase: "merging", total: 2, seen: 1 }, report: null, error: null }) })
      .mockResolvedValueOnce({ ok: true, json: async () => ({ id: 9, kind: "merge_exact", state: "finished", progress: { phase: "merging", total: 2, seen: 2 }, report, error: null }) });
    vi.stubGlobal("fetch", fetchMock);
    const seen: number[] = [];
    const result = await runBulkMerge("abc", true, { intervalMs: 1, onProgress: (j) => seen.push(j.progress.seen) });
    expect(result).toEqual(report);
    expect(seen).toEqual([1, 2]);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/duplicates/merge-exact/default");
    expect(JSON.parse(init.body)).toEqual({ dry_run: false, plan_fingerprint: "abc", keep_duplicate_formats: true });
    expect(fetchMock.mock.calls[1][0]).toBe("/library-jobs/9");
  });

  it("rejects when the server says the plan is stale", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false, status: 409, statusText: "Conflict" }));
    await expect(runBulkMerge("old", false)).rejects.toThrow(/409/);
  });
});
