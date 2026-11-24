import { afterEach, describe, expect, it, vi } from "vitest";

import type { DuplicateBook, DuplicateGroup, ResolvePlan } from "./api";
import { keepAndTrashGroup, keepAndTrashLabel, offerRunUndo, restoredMessage, trashedCopiesMessage } from "./dedupe";
import type { DedupeApi } from "./dedupe";
import { dismissToast, runToastAction, toastState } from "./deleteFlow";

const b = (id: number): DuplicateBook => ({ book_id: id, title: `Book ${id}`, authors: ["x"] });
const group = (ids: number[], id: string | null = "g1"): DuplicateGroup => ({ id: id ?? undefined, reason: "content", books: ids.map(b) });

function plan(keeper: number, others: number[]): ResolvePlan {
  return {
    dry_run: true,
    action: "keep_best_trash_rest",
    plan_fingerprint: "fp",
    groups: [{ group_id: "g1", reason: "content", confidence: 1, action: "keep_best_trash_rest", keeper: b(keeper), keeper_reason: "", others: others.map(b), bytes_freed: 0 }],
    totals: { groups: 1, books: others.length, bytes_freed: 0 },
    skipped: [],
    missing_group_ids: [],
    fully_restorable: true,
    note: "",
  };
}

function fakeApi(over: Partial<DedupeApi> = {}): DedupeApi {
  return {
    planResolve: vi.fn(async () => plan(2, [1, 3])),
    runResolve: vi.fn(async () => ({ run_id: "run-1", action: "keep_best_trash_rest", groups_planned: 1, groups_done: 1, done_groups: ["g1"], books_trashed: 2, bytes_freed: 0, trashed_files: [], errors: [], cancelled: false, fully_restorable: true })),
    restoreResolveRun: vi.fn(async () => ({ run_id: "run-1", restored: [{ book_id: 1, title: "a" }, { book_id: 3, title: "c" }], failed: [], needs_rescan: false })),
    ...over,
  } as DedupeApi;
}

afterEach(() => dismissToast());

describe("keepAndTrashLabel", () => {
  it("counts the others", () => {
    expect(keepAndTrashLabel(group([1, 2]))).toBe("Keep this, trash 1 other");
    expect(keepAndTrashLabel(group([1, 2, 3]))).toBe("Keep this, trash 2 others");
  });
});

describe("keepAndTrashGroup", () => {
  it("plans for exactly this keeper, runs that plan and returns the run to undo", async () => {
    const api = fakeApi();
    const out = await keepAndTrashGroup(group([1, 2, 3]), 2, api);
    expect(out).toEqual({ runId: "run-1", trashedIds: [1, 3] });
    expect(api.planResolve).toHaveBeenCalledWith({ action: "keep_best_trash_rest", groupIds: ["g1"], keepers: { g1: 2 } });
    expect(api.runResolve).toHaveBeenCalledWith({ action: "keep_best_trash_rest", groupIds: ["g1"], keepers: { g1: 2 } }, "fp");
  });

  it("refuses a group the last scan does not have", async () => {
    const api = fakeApi();
    await expect(keepAndTrashGroup(group([1, 2], null), 1, api)).rejects.toThrow(/Rescan/);
    expect(api.planResolve).not.toHaveBeenCalled();
  });

  it("refuses to run a plan for other books than the card shows", async () => {
    const api = fakeApi({ planResolve: vi.fn(async () => plan(2, [1])) });
    await expect(keepAndTrashGroup(group([1, 2, 3]), 2, api)).rejects.toThrow(/exactly these books/);
    expect(api.runResolve).not.toHaveBeenCalled();
  });

  it("returns the run with what went wrong when only some copies were trashed, so its Undo can still be offered", async () => {
    const api = fakeApi({ runResolve: vi.fn(async () => ({ run_id: "r", action: "keep_best_trash_rest", groups_planned: 1, groups_done: 0, done_groups: [], books_trashed: 1, bytes_freed: 0, trashed_files: [], errors: [["g1", "locked"]], cancelled: false, fully_restorable: true })) as unknown as DedupeApi["runResolve"] });
    const out = await keepAndTrashGroup(group([1, 2, 3]), 2, api);
    expect(out).toMatchObject({ runId: "r", partial: true, trashedCount: 1, error: "locked" });
  });

  it("is a plain failure when nothing was trashed", async () => {
    const api = fakeApi({ runResolve: vi.fn(async () => ({ run_id: "r", action: "keep_best_trash_rest", groups_planned: 1, groups_done: 0, done_groups: [], books_trashed: 0, bytes_freed: 0, trashed_files: [], errors: [["g1", "locked"]], cancelled: false, fully_restorable: true })) as unknown as DedupeApi["runResolve"] });
    await expect(keepAndTrashGroup(group([1, 2, 3]), 2, api)).rejects.toThrow("locked");
  });

  it("never offers to trash the only copy", async () => {
    await expect(keepAndTrashGroup(group([1]), 1, fakeApi())).rejects.toThrow(/only copy/);
  });
});

describe("offerRunUndo", () => {
  it("shows a toast whose Undo restores the run and refreshes the caller", async () => {
    const api = fakeApi();
    const refreshed = vi.fn();
    offerRunUndo(trashedCopiesMessage(2, "Dune"), "run-1", refreshed, api);
    expect(toastState.toast?.message).toBe("Kept “Dune” and moved 2 copies to Trash");
    expect(toastState.toast?.actionLabel).toBe("Undo");
    await runToastAction();
    expect(api.restoreResolveRun).toHaveBeenCalledWith("run-1");
    expect(refreshed).toHaveBeenCalled();
    expect(toastState.toast?.result).toBe(restoredMessage(2));
  });

  it("reports why an Undo failed instead of claiming success", async () => {
    const api = fakeApi({ restoreResolveRun: vi.fn(async () => ({ run_id: "r", restored: [], failed: [["1", "gone"]], needs_rescan: false })) as unknown as DedupeApi["restoreResolveRun"] });
    offerRunUndo("x", "r", undefined, api);
    await runToastAction();
    expect(toastState.toast?.result).toBe("gone");
  });
});
