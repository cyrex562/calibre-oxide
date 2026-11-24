import { describe, expect, it, vi } from "vitest";
import { actionNote, createResolveController, defaultDraft, describeReport, draftToSelection, selectionKey } from "./resolve";
import type { ResolveApi } from "./resolve";
import type { ResolvePlan, ResolveReport } from "./api";
import { describePlanTotals, isMetadataOnly, trashConfirmText } from "./duplicates";

const plan = (fingerprint = "fp1"): ResolvePlan => ({
  dry_run: true,
  action: "keep_best_trash_rest",
  plan_fingerprint: fingerprint,
  groups: [],
  totals: { groups: 3, books: 4, bytes_freed: 3 * 1024 * 1024 * 1024 },
  skipped: [],
  missing_group_ids: [],
  fully_restorable: true,
  note: "",
});

const report = (over: Partial<ResolveReport> = {}): ResolveReport => ({ run_id: "run-1", action: "keep_best_trash_rest", groups_planned: 3, groups_done: 3, done_groups: ["a", "b", "c"], books_trashed: 4, bytes_freed: 1024, trashed_files: [], errors: [], cancelled: false, fully_restorable: true, ...over });

function fakeApi(over: Partial<ResolveApi> = {}): ResolveApi {
  return {
    planResolve: vi.fn(async () => plan()),
    runResolve: vi.fn(async () => report()),
    fetchResolveRuns: vi.fn(async () => ({ runs: [{ run_id: "run-1", books: 4, bytes: 1024, deleted_at: 1 }] })),
    restoreResolveRun: vi.fn(async () => ({ run_id: "run-1", restored: [{ book_id: 1, title: "x" }], failed: [], needs_rescan: true })),
    ...over,
  };
}

describe("the draft", () => {
  it("starts with identical files only, never a title tier", () => {
    const d = defaultDraft();
    expect(d.tiers).toEqual(["content"]);
    expect(d.tiers.some(isMetadataOnly)).toBe(false);
    expect(d.action).toBe("keep_best_trash_rest");
  });

  it("is sent to the server as a selection, leaving empty fields out", () => {
    const sel = draftToSelection({ ...defaultDraft(), minConfidence: 0.9, pathPrefix: " Fiction/ ", tiers: ["content", "pdf_resave"] }, { g1: 5 });
    expect(sel).toMatchObject({ action: "keep_best_trash_rest", tiers: ["content", "pdf_resave"], minConfidence: 0.9, pathPrefix: "Fiction/", keepers: { g1: 5 } });
    const bare = draftToSelection(defaultDraft());
    expect(bare.minConfidence).toBeUndefined();
    expect(bare.pathPrefix).toBeUndefined();
  });

  it("can ignore the person's keeper picks", () => {
    expect(draftToSelection({ ...defaultDraft(), useMyKeepers: false }, { g1: 5 }).keepers).toBeUndefined();
  });

  it("keys a selection by its content, whatever the order of tiers", () => {
    const a = selectionKey(draftToSelection({ ...defaultDraft(), tiers: ["content", "pdf_resave"] }));
    const b = selectionKey(draftToSelection({ ...defaultDraft(), tiers: ["pdf_resave", "content"] }));
    expect(a).toBe(b);
    expect(a).not.toBe(selectionKey(draftToSelection({ ...defaultDraft(), action: "merge_as_formats", tiers: ["content", "pdf_resave"] })));
  });
});

describe("the dialog's flow", () => {
  it("will not run without a preview", async () => {
    const api = fakeApi();
    const c = createResolveController(api);
    expect(await c.run()).toBeNull();
    expect(c.state.error).toMatch(/preview/i);
    expect(api.runResolve).not.toHaveBeenCalled();
  });

  it("previews, then runs exactly that plan with its fingerprint", async () => {
    const api = fakeApi();
    const c = createResolveController(api);
    await c.preview({ g1: 9 });
    expect(c.state.phase).toBe("planned");
    expect(c.state.plan?.plan_fingerprint).toBe("fp1");
    const done = await c.run({ g1: 9 });
    expect(done?.run_id).toBe("run-1");
    expect(c.state.phase).toBe("done");
    expect(vi.mocked(api.runResolve).mock.calls[0][1]).toBe("fp1");
    expect(vi.mocked(api.runResolve).mock.calls[0][0].keepers).toEqual({ g1: 9 });
  });

  it("an edit after the preview voids it", async () => {
    const api = fakeApi();
    const c = createResolveController(api);
    await c.preview();
    c.state.draft.tiers = ["content", "title_author_exact"];
    c.edited();
    expect(c.state.plan).toBeNull();
    expect(c.state.phase).toBe("idle");
    expect(await c.run()).toBeNull();
    expect(api.runResolve).not.toHaveBeenCalled();
  });

  it("a changed keeper pick since the preview is also not the previewed plan", async () => {
    const c = createResolveController(fakeApi());
    await c.preview({ g1: 9 });
    expect(c.planIsCurrent({ g1: 9 })).toBe(true);
    expect(c.planIsCurrent({ g1: 8 })).toBe(false);
    expect(await c.run({ g1: 8 })).toBeNull();
  });

  it("a refusal (409: the library changed) voids the plan and shows the error", async () => {
    const api = fakeApi({ runResolve: vi.fn().mockRejectedValue(new Error("POST /duplicates/resolve/default failed: 409 Conflict")) });
    const c = createResolveController(api);
    await c.preview();
    expect(await c.run()).toBeNull();
    expect(c.state.error).toMatch(/409/);
    expect(c.state.plan).toBeNull();
    expect(c.state.phase).toBe("idle");
  });

  it("a failed preview leaves no plan", async () => {
    const c = createResolveController(fakeApi({ planResolve: vi.fn().mockRejectedValue(new Error("scan first")) }));
    await c.preview();
    expect(c.state.plan).toBeNull();
    expect(c.state.error).toBe("scan first");
  });

  it("follows the job's progress", async () => {
    const lines: string[] = [];
    const api = fakeApi({
      runResolve: vi.fn(async (_s, _f, handlers) => {
        handlers?.onStart?.(4);
        handlers?.onProgress?.({ id: 4, kind: "resolve", state: "running", progress: { phase: "resolving", total: 3, seen: 1, hashed: 0, added: 0, already_known: 0, ignored: 0, enriched: 0, errors: 0, bytes_hashed: 0, cancelled: false }, report: null, error: null });
        lines.push(c.state.progress);
        return report();
      }),
    });
    const c = createResolveController(api);
    await c.preview();
    await c.run();
    expect(lines).toEqual(["Resolving… 1 of 3 groups"]);
  });

  it("undoes a run and forgets it", async () => {
    const api = fakeApi();
    const c = createResolveController(api);
    await c.loadRuns();
    expect(c.state.runs).toHaveLength(1);
    await c.preview();
    await c.run();
    expect(await c.undo("run-1")).toBe(true);
    expect(api.restoreResolveRun).toHaveBeenCalledWith("run-1");
    expect(c.state.undone).toBe("1 book restored.");
    expect(c.state.runs).toEqual([]);
    expect(c.state.report).toBeNull();
  });
});

describe("how things are described", () => {
  it("never calls a merge fully undoable", () => {
    expect(actionNote("keep_best_trash_rest")).toMatch(/restored/);
    expect(actionNote("merge_as_formats")).toMatch(/does not take what was merged out/i);
  });

  it("summarises a run's report and its problems", () => {
    expect(describeReport(report())).toBe("3 of 3 groups resolved · 4 books moved to the trash · about 1 KB in the Trash (freed once the Trash is emptied or expires)");
    const bad = describeReport(report({ groups_done: 2, cancelled: true, errors: [["abcdef123", "the keeper has no file"]] }));
    expect(bad).toContain("2 of 3 groups resolved");
    expect(bad).toContain("stopped early");
    expect(bad).toContain("1 problem: abcdef… the keeper has no file");
    expect(describeReport(report({ action: "merge_as_formats" }))).toContain("merged and trashed");
  });

  it("totals a plan in groups, books and gigabytes", () => {
    expect(describePlanTotals({ groups: 12, books: 14, bytes_freed: 1.5 * 1024 ** 3 }, "keep_best_trash_rest")).toBe("12 groups · 14 books moved to the trash · about 1.5 GB to the Trash (freed once the Trash is emptied or expires)");
    expect(describePlanTotals({ groups: 1, books: 1, bytes_freed: 10 }, "merge_as_formats")).toBe("1 group · 1 book merged into their keepers · about 10 B to the Trash (freed once the Trash is emptied or expires)");
  });

  it("says that delete goes to the trash and can be undone", () => {
    const text = trashConfirmText("Dune");
    expect(text).toMatch(/library trash/);
    expect(text).toMatch(/restored/);
    expect(text).not.toMatch(/cannot be undone/);
  });
});
