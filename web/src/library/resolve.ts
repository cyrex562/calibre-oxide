// The "Resolve…" dialog: choose which duplicate groups to act on and how, see
// the plan, run it, undo it.
//
// The server is the authority (crates/lexmechanic_srv/src/duplicates_review.rs):
// it picks the groups, picks each group's keeper and says why, and refuses to
// execute a plan the person has not previewed. This module holds the dialog's
// state machine so its rules can be tested:
//
// * a run needs a preview of exactly this selection (any edit drops the preview);
// * title-only tiers start unticked and stay unticked;
// * a merge is never described as fully undoable.

import { reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { DuplicateReason, ResolveAction, ResolvePlan, ResolveReport, ResolveRun, ResolveSelection } from "./api";
import { DEFAULT_RESOLVE_TIERS, formatBytes } from "./duplicates";

export type ResolveApi = Pick<typeof realApi, "planResolve" | "runResolve" | "fetchResolveRuns" | "restoreResolveRun">;

export interface ResolveDraft {
  tiers: DuplicateReason[];
  /** 0 to 1. */
  minConfidence: number;
  pathPrefix: string;
  action: ResolveAction;
  keepDuplicateFormats: boolean;
  /** Send the keeper the person chose in the list instead of the suggestion, where they chose one. */
  useMyKeepers: boolean;
}

export function defaultDraft(): ResolveDraft {
  return { tiers: [...DEFAULT_RESOLVE_TIERS], minConfidence: 0, pathPrefix: "", action: "keep_best_trash_rest", keepDuplicateFormats: false, useMyKeepers: true };
}

/** What the server is asked for. `keepers` are the person's picks by group id. */
export function draftToSelection(d: ResolveDraft, keepers: Record<string, number> = {}): ResolveSelection {
  return {
    action: d.action,
    tiers: d.tiers,
    minConfidence: d.minConfidence > 0 ? d.minConfidence : undefined,
    pathPrefix: d.pathPrefix.trim() || undefined,
    keepers: d.useMyKeepers ? keepers : undefined,
    keepDuplicateFormats: d.keepDuplicateFormats,
  };
}

/** Identifies a selection, so an edit after the preview can be told from no edit. */
export function selectionKey(sel: ResolveSelection): string {
  return JSON.stringify([sel.action, [...(sel.tiers ?? [])].sort(), sel.minConfidence ?? 0, sel.pathPrefix ?? "", sel.groupIds ?? null, sel.keepers ?? {}, sel.keepDuplicateFormats ?? false]);
}

/** What the action does, for the dialog and the confirmation. */
export function actionNote(action: ResolveAction): string {
  return action === "keep_best_trash_rest"
    ? "Each group keeps its suggested copy; the other books are moved to the library trash (Library → Trash…), from where this run or any single book can be restored."
    : "Each group keeps its suggested copy, and the other books are merged into it (their formats, tags and other missing details are copied in) and then trashed. Undoing the run brings the trashed books back but does NOT take what was merged out of the kept copy.";
}

export function describeReport(r: ResolveReport): string {
  const n = (v: number) => v.toLocaleString("en-US");
  const parts = [`${n(r.groups_done)} of ${n(r.groups_planned)} groups resolved`, `${n(r.books_trashed)} book${r.books_trashed === 1 ? "" : "s"} ${r.action === "merge_as_formats" ? "merged and trashed" : "moved to the trash"}`, `about ${formatBytes(r.bytes_freed)} in the Trash (freed once the Trash is emptied or expires)`];
  if (r.cancelled) parts.push("stopped early");
  if (r.errors.length > 0) parts.push(`${n(r.errors.length)} problem${r.errors.length === 1 ? "" : "s"}: ${r.errors.slice(0, 2).map(([g, why]) => `${g.slice(0, 6)}… ${why}`).join("; ")}`);
  return parts.join(" · ");
}

export type ResolvePhase = "idle" | "planning" | "planned" | "running" | "done";

export function createResolveController(api: ResolveApi = realApi) {
  const state = reactive({
    draft: defaultDraft(),
    phase: "idle" as ResolvePhase,
    plan: null as ResolvePlan | null,
    /** The selection the plan was made for. */
    planKey: "",
    report: null as ResolveReport | null,
    progress: "",
    jobId: null as number | null,
    error: null as string | null,
    undone: null as string | null,
    runs: [] as ResolveRun[],
  });

  /** Whether the preview on screen is for what is selected now. */
  function planIsCurrent(keepers: Record<string, number> = {}): boolean {
    return state.plan !== null && state.planKey === selectionKey(draftToSelection(state.draft, keepers));
  }

  /** An edit invalidates the preview (the run button needs one for exactly this selection). */
  function edited(): void {
    if (state.phase === "planned") {
      state.plan = null;
      state.phase = "idle";
    }
  }

  async function preview(keepers: Record<string, number> = {}): Promise<void> {
    const sel = draftToSelection(state.draft, keepers);
    state.phase = "planning";
    state.error = null;
    state.report = null;
    state.undone = null;
    try {
      state.plan = await api.planResolve(sel);
      state.planKey = selectionKey(sel);
      state.phase = "planned";
    } catch (e) {
      state.plan = null;
      state.phase = "idle";
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** Runs the previewed plan. Refuses without a preview of the current selection. */
  async function run(keepers: Record<string, number> = {}): Promise<ResolveReport | null> {
    const plan = state.plan;
    if (!plan || state.phase !== "planned" || !planIsCurrent(keepers)) {
      state.error = "Preview the plan first.";
      return null;
    }
    const sel = draftToSelection(state.draft, keepers);
    state.phase = "running";
    state.error = null;
    state.progress = "Starting…";
    try {
      const report = await api.runResolve(sel, plan.plan_fingerprint, {
        onStart: (id) => (state.jobId = id),
        onProgress: (job) => (state.progress = describeJobProgress(job.progress)),
      });
      state.report = report;
      state.phase = "done";
      state.plan = null;
      return report;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      // A 409 means the library or the selection moved on: the preview is void.
      state.plan = null;
      state.phase = "idle";
      return null;
    } finally {
      state.jobId = null;
      state.progress = "";
    }
  }

  async function loadRuns(): Promise<void> {
    try {
      state.runs = (await api.fetchResolveRuns()).runs;
    } catch {
      state.runs = [];
    }
  }

  /** Puts back everything the run trashed. */
  async function undo(runId: string): Promise<boolean> {
    state.error = null;
    try {
      const done = await api.restoreResolveRun(runId);
      state.undone = `${done.restored.length} book${done.restored.length === 1 ? "" : "s"} restored${done.failed.length ? `, ${done.failed.length} could not be` : ""}.`;
      state.runs = state.runs.filter((r) => r.run_id !== runId);
      if (state.report?.run_id === runId) state.report = null;
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  function reset(): void {
    state.phase = "idle";
    state.plan = null;
    state.report = null;
    state.error = null;
    state.undone = null;
  }

  return { state, planIsCurrent, edited, preview, run, loadRuns, undo, reset };
}

export type ResolveController = ReturnType<typeof createResolveController>;
