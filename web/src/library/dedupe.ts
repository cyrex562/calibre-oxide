// "Keep this, trash the others" for one duplicate group, and the one Undo toast
// every de-dup path shows afterwards (tranche 26: U4, U5).
//
// There are four ways to get rid of extra copies: this one-click action on a
// group card, Compare's "Keep selected, trash the others", Resolve (many groups
// at once) and a plain Delete on one copy. They all end the same way: a toast
// that says what happened, with an Undo that puts the books back. The first
// three go through the resolve route, so they share one undo, the resolve run's
// restore; a plain Delete uses the Trash restore (./deleteFlow.ts).
//
// Merging books into one record cannot be undone: the merged-away book rows are
// deleted, not trashed. Its toast says so instead of offering an Undo that
// would not work.

import * as realApi from "./api";
import type { DuplicateGroup, ResolveSelection } from "./api";
import { showToast } from "./deleteFlow";

export type DedupeApi = Pick<typeof realApi, "planResolve" | "runResolve" | "restoreResolveRun">;

const n = (v: number) => v.toLocaleString("en-US");

/** The label of the group card's primary button: "Keep this, trash 2 others". */
export function keepAndTrashLabel(group: Pick<DuplicateGroup, "books">): string {
  const others = Math.max(0, group.books.length - 1);
  return `Keep this, trash ${others === 1 ? "1 other" : `${n(others)} others`}`;
}

/** The toast text after trashing `count` copies of one book. */
export function trashedCopiesMessage(count: number, keeperTitle: string): string {
  const title = keeperTitle ? ` “${keeperTitle}”` : "";
  return `Kept${title} and moved ${n(count)} ${count === 1 ? "copy" : "copies"} to Trash`;
}

/** The toast text after Restore. */
export function restoredMessage(count: number): string {
  return `Restored ${n(count)} ${count === 1 ? "book" : "books"}`;
}

export interface KeepTrashResult {
  runId: string;
  trashedIds: number[];
}

/**
 * Keeps `keeperId` and moves the group's other books to the library Trash, as
 * one resolve run (so it can be undone). The server plans it from the scan
 * snapshot; this refuses to run a plan that is not for exactly this keeper and
 * these others. Throws with the server's words when it cannot.
 */
export async function keepAndTrashGroup(group: Pick<DuplicateGroup, "id" | "books">, keeperId: number, api: DedupeApi = realApi): Promise<KeepTrashResult> {
  if (!group.id) throw new Error("This group is not in the last scan, so it cannot be resolved here. Rescan first.");
  if (!group.books.some((b) => b.book_id === keeperId)) throw new Error("The copy to keep is not in this group.");
  const others = group.books.map((b) => b.book_id).filter((id) => id !== keeperId);
  if (others.length === 0) throw new Error("There is nothing to trash: this is the only copy.");
  const selection: ResolveSelection = { action: "keep_best_trash_rest", groupIds: [group.id], keepers: { [group.id]: keeperId } };
  const plan = await api.planResolve(selection);
  const planned = plan.groups[0];
  const plannedOthers = (planned?.others ?? []).map((b) => b.book_id).sort((a, b) => a - b);
  if (plan.groups.length !== 1 || !planned || planned.keeper.book_id !== keeperId || plannedOthers.join(",") !== [...others].sort((a, b) => a - b).join(",")) {
    throw new Error(plan.skipped[0]?.reason ?? "The server's plan is not for exactly these books, so nothing was trashed. Rescan and try again.");
  }
  const report = await api.runResolve(selection, plan.plan_fingerprint);
  if (report.books_trashed < others.length) throw new Error(report.errors[0]?.[1] ?? "Not every other copy could be trashed.");
  return { runId: report.run_id, trashedIds: others };
}

/**
 * The one de-dup toast: what happened, and an Undo that restores the resolve
 * run `runId`. `onRestored` runs after a successful Undo so the caller can
 * refresh its lists.
 */
export function offerRunUndo(message: string, runId: string, onRestored?: () => void | Promise<void>, api: Pick<DedupeApi, "restoreResolveRun"> = realApi): void {
  showToast(message, {
    label: "Undo",
    run: async () => {
      const done = await api.restoreResolveRun(runId);
      if (done.restored.length === 0) throw new Error(done.failed[0]?.[1] ?? "Nothing could be restored.");
      await onRestored?.();
      return restoredMessage(done.restored.length);
    },
  });
}
