// The "Organize files…" dialog: move each book's own files inside the library
// to where a template says, see the plan, run it, undo it.
//
// The server is the authority (crates/lexmechanic_srv/src/organize_library.rs): it
// plans the moves, never overwrites (a taken name gets " (2)"), and refuses to
// execute a plan that was not previewed. This module holds the dialog's state
// machine so its rules can be tested:
//
// * a run needs a preview of exactly this template and scope (any edit drops it);
// * the scope is the selected books when there are any, else the books of the
//   current search or filter when one is active, else everything. A "search"
//   scope is sent as the ids of the books the list shows (the dialog passes them
//   where it passes the selection), so it covers every kind of filter: a search,
//   a virtual library, a category pick, the marked books.

import { reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { OrganizePlan, OrganizeReport, OrganizeRun, OrganizeScope, OrganizeUndoReport } from "./api";

export type OrganizeApi = Pick<typeof realApi, "planOrganize" | "runOrganize" | "fetchOrganizeRuns" | "undoOrganize">;

export const DEFAULT_ORGANIZE_TEMPLATE = "{author_sort}/{title}";

export type OrganizeScopeKind = "selected" | "search" | "folder" | "all";

export interface OrganizeDraft {
  template: string;
  scope: OrganizeScopeKind;
  /** Relative to the library, for the "folder" scope. */
  pathPrefix: string;
}

/** The scope a dialog opens with: the selection when there is one, else the current search when one is active. */
export function defaultDraft(selectedCount = 0, searchActive = false): OrganizeDraft {
  return { template: DEFAULT_ORGANIZE_TEMPLATE, scope: selectedCount > 0 ? "selected" : searchActive ? "search" : "all", pathPrefix: "" };
}

/**
 * What the server is asked to cover. `ids` are the books of the "selected" or
 * "search" scope: the dialog passes the selection for one and the current
 * search's books for the other.
 */
export function draftToScope(d: OrganizeDraft, selectedIds: number[]): OrganizeScope {
  if (d.scope === "selected" || d.scope === "search") return { bookIds: [...selectedIds] };
  if (d.scope === "folder") return { pathPrefix: d.pathPrefix.trim().replace(/^[\\/]+|[\\/]+$/g, "") };
  return {};
}

/** Whether the draft can be previewed at all. */
export function draftProblem(d: OrganizeDraft, selectedIds: number[]): string | null {
  if (d.scope === "selected" && selectedIds.length === 0) return "No books are selected.";
  if (d.scope === "search" && selectedIds.length === 0) return "No books match the current search.";
  if (d.scope === "folder" && d.pathPrefix.trim() === "") return "Enter the folder to organize.";
  return null;
}

/** Identifies a template and scope, so an edit after the preview can be told from no edit. */
export function planKey(d: OrganizeDraft, selectedIds: number[]): string {
  const scope = draftToScope(d, selectedIds);
  return JSON.stringify([d.template.trim(), scope.bookIds ? [...scope.bookIds].sort((a, b) => a - b) : null, scope.pathPrefix ?? null]);
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (v: number, one: string, many = `${one}s`) => `${n(v)} ${v === 1 ? one : many}`;

/** The totals line of a preview. */
export function describePlan(p: OrganizePlan): string {
  const parts = [`${plural(p.books, "book")} (${plural(p.files, "file")}) would move`];
  if (p.already_organized > 0) parts.push(`${n(p.already_organized)} already in place`);
  if (p.conflicts > 0) parts.push(`${plural(p.conflicts, "name")} taken, so numbered`);
  if (p.skipped.length > 0) parts.push(`${n(p.skipped.length)} skipped`);
  if (p.data_folders) parts.push(`${plural(p.data_folders, "attachments folder")} move${p.data_folders === 1 ? "s" : ""} too`);
  if (p.warnings?.some((w) => w.reason.includes("stay where they are"))) parts.push("some attachments stay behind");
  return parts.join(" · ");
}

/** True when the list is shorter than the totals (the server caps the sample). */
export function planListIsPartial(p: OrganizePlan): boolean {
  return p.moves.length < p.files;
}

export function describeReport(r: OrganizeReport): string {
  const parts = [`${plural(r.books_moved, "book")} of ${n(r.books_planned)} organized`, `${plural(r.files_moved, "file")} moved`];
  if (r.cancelled) parts.push("stopped early");
  if (r.failed.length > 0) parts.push(`${n(r.failed.length)} could not be moved and ${r.failed.length === 1 ? "was" : "were"} left where ${r.failed.length === 1 ? "it" : "they"} were`);
  return parts.join(" · ");
}

/**
 * One line saying what an earlier run did, so a drag-move and a folder rename
 * (both "1 book") can be told apart in the list.
 */
export function describeRun(r: Pick<OrganizeRun, "books" | "template">): string {
  const books = plural(r.books, "book");
  const t = (r.template ?? "").trim();
  const move = /^move to (.+)$/.exec(t);
  if (move) return `Moved ${books} to ${move[1]}`;
  const rename = /^rename folder (.+) to (.+)$/.exec(t);
  if (rename) return `Renamed folder ${rename[1]} → ${rename[2]} (${books})`;
  if (t) return `Organized ${books} with ${t}`;
  return books;
}

export function describeUndo(r: OrganizeUndoReport): string {
  const parts = [`${plural(r.books_restored, "book")} put back (${plural(r.files_restored, "file")})`];
  if (r.skipped.length > 0) parts.push(`${n(r.skipped.length)} left alone because they changed since`);
  return parts.join(" · ");
}

export type OrganizePhase = "idle" | "planning" | "planned" | "running" | "done";

export function createOrganizeController(api: OrganizeApi = realApi) {
  const state = reactive({
    draft: defaultDraft(),
    phase: "idle" as OrganizePhase,
    plan: null as OrganizePlan | null,
    /** The template and scope the plan was made for. */
    planKey: "",
    report: null as OrganizeReport | null,
    progress: "",
    jobId: null as number | null,
    error: null as string | null,
    undone: null as string | null,
    runs: [] as OrganizeRun[],
  });

  /** Opens fresh: scope follows the selection, else the active search; nothing previewed. */
  function open(selectedCount: number, searchActive = false): void {
    state.draft = defaultDraft(selectedCount, searchActive);
    state.phase = "idle";
    state.plan = null;
    state.planKey = "";
    state.report = null;
    state.error = null;
    state.undone = null;
    state.progress = "";
    state.jobId = null;
  }

  /** Whether the preview on screen is for what is chosen now. */
  function planIsCurrent(selectedIds: number[]): boolean {
    return state.plan !== null && state.planKey === planKey(state.draft, selectedIds);
  }

  /** An edit invalidates the preview (the run button needs one for exactly this choice). */
  function edited(): void {
    if (state.phase === "planned") {
      state.plan = null;
      state.phase = "idle";
    }
  }

  async function preview(selectedIds: number[]): Promise<void> {
    const problem = draftProblem(state.draft, selectedIds);
    if (problem) {
      state.error = problem;
      return;
    }
    const key = planKey(state.draft, selectedIds);
    state.phase = "planning";
    state.error = null;
    state.report = null;
    state.undone = null;
    try {
      state.plan = await api.planOrganize(state.draft.template, draftToScope(state.draft, selectedIds));
      state.planKey = key;
      state.phase = "planned";
    } catch (e) {
      state.plan = null;
      state.phase = "idle";
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** Runs the previewed plan. Refuses without a preview of the current choice. */
  async function run(selectedIds: number[]): Promise<OrganizeReport | null> {
    const plan = state.plan;
    if (!plan || state.phase !== "planned" || !planIsCurrent(selectedIds)) {
      state.error = "Preview first.";
      return null;
    }
    state.phase = "running";
    state.error = null;
    state.progress = "Starting…";
    try {
      const report = await api.runOrganize(state.draft.template, draftToScope(state.draft, selectedIds), plan.plan_fingerprint, {
        onStart: (id) => (state.jobId = id),
        onProgress: (job) => (state.progress = describeJobProgress(job.progress)),
      });
      state.report = report;
      state.phase = "done";
      state.plan = null;
      await loadRuns();
      return report;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      // A 409 means the library or the scope moved on: the preview is void.
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
      state.runs = (await api.fetchOrganizeRuns()).runs.filter((r) => !r.undone);
    } catch {
      state.runs = [];
    }
  }

  /** Moves a run's files back. */
  async function undo(runId: string): Promise<boolean> {
    state.error = null;
    state.progress = "Putting files back…";
    state.phase = "running";
    try {
      const done = await api.undoOrganize(runId, { onStart: (id) => (state.jobId = id) });
      state.undone = describeUndo(done);
      state.runs = state.runs.filter((r) => r.run_id !== runId);
      if (state.report?.run_id === runId) state.report = null;
      state.phase = "idle";
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      state.phase = state.report ? "done" : "idle";
      return false;
    } finally {
      state.jobId = null;
      state.progress = "";
    }
  }

  return { state, open, planIsCurrent, edited, preview, run, loadRuns, undo };
}

export type OrganizeController = ReturnType<typeof createOrganizeController>;
