// The "Fix titles…" dialog: find books whose stored title is junk, suggest a
// clean one, let the user approve or edit each, then in one run set the title,
// optionally write it into the book files and optionally rename the files from
// a template. A run can be undone.
//
// The server is the authority (crates/lexmechanic_srv/src/retitle_library.rs): it
// finds and suggests (dry run), skips a book whose title changed since the
// preview, isolates errors per book and records every run. This module holds the
// dialog's state machine so its rules can be tested:
//
// * a row is pre-checked when its title needs fixing and a suggestion that
//   reads like a title exists (not a catalogue number such as `MR1494.ch4`);
// * the table is shown in pages (thousands of rows must stay usable);
// * a checked row with an empty title cannot be applied;
// * the header checkbox and the reason filter act on every row of the filter, not just the page.

import { reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { RetitleItem, RetitleReason, RetitleReport, RetitleRow, RetitleRun, RetitleScope, RetitleUndoReport } from "./api";

export type RetitleApi = Pick<typeof realApi, "suggestRetitles" | "runRetitle" | "fetchRetitleRuns" | "undoRetitle">;

export const DEFAULT_RETITLE_TEMPLATE = "{title}";
export const RETITLE_PAGE_SIZE = 200;

export type RetitleScopeKind = "all" | "selected" | "search";
export type RetitleFilter = "all" | RetitleReason;
export type RetitlePhase = "idle" | "finding" | "ready" | "running" | "done";

/** One table row: the server's suggestion plus what the user did with it. */
export interface RetitleRowState {
  bookId: number;
  currentTitle: string;
  /** What will be applied: the suggestion until the user edits it. */
  title: string;
  reason: RetitleReason;
  currentFile: string;
  proposedFile: string | null;
  formats: string[];
  /** The suggestion reads like a title (see the server's `looks_like_title`). */
  confident: boolean;
  checked: boolean;
}

/** A row is pre-checked when its title is wrong and a confident suggestion exists. */
export function rowFromServer(r: RetitleRow): RetitleRowState {
  return {
    bookId: r.book_id,
    currentTitle: r.current_title,
    title: r.suggested_title,
    reason: r.reason,
    currentFile: r.current_file,
    proposedFile: r.proposed_file,
    formats: r.formats,
    confident: r.confident !== false,
    checked: r.reason !== "ok" && r.reason !== "no_suggestion" && r.confident !== false && r.suggested_title.trim() !== "",
  };
}

/**
 * Ready-to-apply rows first, then suggestions to check, then books with no
 * suggestion, then the fine ones; stable within each, so the server's order
 * holds otherwise. A first page of "Type a title" rows hid the 600 good ones.
 */
export function orderRows(rows: RetitleRowState[]): RetitleRowState[] {
  const rank = (r: RetitleRowState) => (r.checked ? 0 : r.reason === "ok" ? 3 : r.reason === "no_suggestion" ? 2 : 1);
  return rows.map((r, i) => [r, i] as const).sort((a, b) => rank(a[0]) - rank(b[0]) || a[1] - b[1]).map(([r]) => r);
}

export const REASON_LABEL: Record<RetitleReason, string> = {
  junk: "Junk title",
  template_leftover: "Template leftover",
  ok: "Fine",
  no_suggestion: "Needs a title",
};

/** `ids` are the selection for "selected" and the current search's books for "search". */
export function scopeFor(kind: RetitleScopeKind, selectedIds: number[]): RetitleScope {
  return kind === "selected" || kind === "search" ? { bookIds: [...selectedIds] } : { all: true };
}

export function visibleRows(rows: RetitleRowState[], filter: RetitleFilter): RetitleRowState[] {
  return filter === "all" ? rows : rows.filter((r) => r.reason === filter);
}

export function pageCount(total: number, pageSize = RETITLE_PAGE_SIZE): number {
  return Math.max(1, Math.ceil(total / pageSize));
}

export function pageSlice<T>(rows: T[], page: number, pageSize = RETITLE_PAGE_SIZE): T[] {
  const p = Math.min(Math.max(0, page), pageCount(rows.length, pageSize) - 1);
  return rows.slice(p * pageSize, (p + 1) * pageSize);
}

/** `none`, `some` or `all` of these rows are checked (for the header checkbox). */
export function checkState(rows: RetitleRowState[]): "none" | "some" | "all" {
  const n = rows.filter((r) => r.checked).length;
  return n === 0 ? "none" : n === rows.length ? "all" : "some";
}

/** Why a checked row cannot be applied, if it cannot. */
export function rowProblem(r: RetitleRowState): string | null {
  return r.checked && r.title.trim() === "" ? "Type a title for this book, or untick it." : null;
}

/** What the server is asked to apply: every checked row that has a title. */
export function itemsToApply(rows: RetitleRowState[]): RetitleItem[] {
  return rows.filter((r) => r.checked && r.title.trim() !== "").map((r) => ({ book_id: r.bookId, expected_title: r.currentTitle, title: r.title.trim() }));
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (v: number, one: string, many = `${one}s`) => `${n(v)} ${v === 1 ? one : many}`;

export interface RetitleChoices {
  embed: boolean;
  renameFiles: boolean;
  renameTemplate: string;
}

/** The confirmation text: what will happen, in the user's terms. */
export function describeConfirm(count: number, c: RetitleChoices): string {
  const parts = [plural(count, "title")];
  parts.push(c.embed ? "files rewritten" : "files left as they are");
  parts.push(c.renameFiles ? "files renamed" : "files not renamed");
  return `${parts.join(", ")}. You can undo this run afterwards.`;
}

/** The line under the table: how many rows, how many ticked. */
export function describeRows(shown: number, loaded: number, checked: number, serverTotal: number, truncated: boolean): string {
  const parts = [`${plural(shown, "book")}${shown !== loaded ? ` of ${n(loaded)}` : ""}`, `${n(checked)} selected`];
  if (truncated) parts.push(`showing the first ${n(loaded)} of ${n(serverTotal)} that need fixing`);
  return parts.join(" · ");
}

const SKIP_REASON: Record<string, string> = {
  changed_since_preview: "its title changed since the preview",
  invalid: "the new title is empty",
  no_such_book: "it is no longer in the library",
};
export const describeSkip = (reason: string): string => SKIP_REASON[reason] ?? reason;

const STAGE: Record<string, string> = { title: "setting the title", embed: "writing it into the files", rename: "renaming the files" };

export function describeFailure(f: { stage: string; reason: string; title_set: boolean }): string {
  return `${STAGE[f.stage] ?? f.stage} failed${f.title_set ? " (the new title was kept)" : ""}: ${f.reason}`;
}

export function describeReport(r: RetitleReport): string {
  const parts = [`${plural(r.retitled, "title")} set`];
  if (r.embedded > 0) parts.push(`${plural(r.embedded, "book")} rewritten`);
  if (r.renamed > 0) parts.push(`${plural(r.renamed, "book")} renamed`);
  if (r.skipped.length > 0) parts.push(`${n(r.skipped.length)} skipped`);
  if (r.failed.length > 0) parts.push(`${n(r.failed.length)} with problems`);
  if (r.cancelled) parts.push("stopped early");
  return parts.join(" · ");
}

export function describeUndo(r: RetitleUndoReport): string {
  const parts = [`${plural(r.titles_restored, "title")} put back`, `${plural(r.files_restored, "file")} renamed back`];
  if (r.re_embedded > 0) parts.push(`${plural(r.re_embedded, "book")} rewritten again`);
  if (r.skipped.length > 0) parts.push(`${n(r.skipped.length)} left alone because they changed since`);
  if (r.failed.length > 0) parts.push(`${n(r.failed.length)} with problems`);
  return parts.join(" · ");
}

export function createRetitleController(api: RetitleApi = realApi) {
  const state = reactive({
    scope: "all" as RetitleScopeKind,
    embed: true,
    renameFiles: true,
    renameTemplate: DEFAULT_RETITLE_TEMPLATE,
    phase: "idle" as RetitlePhase,
    rows: [] as RetitleRowState[],
    /** Books needing a fix in the scope (the rows may be fewer: the server caps them). */
    serverTotal: 0,
    truncated: false,
    filter: "all" as RetitleFilter,
    page: 0,
    /** The template the proposed file names were computed for. */
    templateAtFind: "",
    report: null as RetitleReport | null,
    undone: null as string | null,
    progress: "",
    jobId: null as number | null,
    error: null as string | null,
    runs: [] as RetitleRun[],
  });

  /** Changes which books are looked at; the rows found for the old scope no longer apply. */
  function setScope(scope: RetitleScopeKind): void {
    if (state.scope === scope) return;
    state.scope = scope;
    if (state.phase === "ready" || state.phase === "idle") {
      state.phase = "idle";
      state.rows = [];
      state.serverTotal = 0;
      state.truncated = false;
      state.filter = "all";
      state.page = 0;
      state.error = null;
    }
  }

  /** Opens fresh. "selected" with nothing selected falls back to every book. */
  function open(scope: RetitleScopeKind): void {
    state.scope = scope;
    state.phase = "idle";
    state.rows = [];
    state.serverTotal = 0;
    state.truncated = false;
    state.filter = "all";
    state.page = 0;
    state.templateAtFind = "";
    state.report = null;
    state.undone = null;
    state.progress = "";
    state.jobId = null;
    state.error = null;
  }

  const filtered = () => visibleRows(state.rows, state.filter);

  /** The rows on screen: the filter's rows, one page. */
  function pageRows(): RetitleRowState[] {
    return pageSlice(filtered(), state.page);
  }

  function setFilter(filter: RetitleFilter): void {
    state.filter = filter;
    state.page = 0;
  }

  function setPage(page: number): void {
    state.page = Math.min(Math.max(0, page), pageCount(filtered().length) - 1);
  }

  /** The header checkbox: tick or untick every row of the current filter (not just the page). */
  function setAllChecked(checked: boolean): void {
    for (const r of filtered()) r.checked = checked && r.title.trim() !== "";
  }

  /** The count of ticked rows, anywhere in the list. */
  function checkedCount(): number {
    return state.rows.filter((r) => r.checked).length;
  }

  /** Whether the file names on screen were computed for another template than the one typed now. */
  function proposedIsStale(): boolean {
    return state.rows.length > 0 && state.templateAtFind !== state.renameTemplate.trim();
  }

  /** Whether the run button is usable. */
  function canApply(): boolean {
    if (state.phase !== "ready") return false;
    const checked = state.rows.filter((r) => r.checked);
    return checked.length > 0 && checked.every((r) => rowProblem(r) === null);
  }

  async function find(selectedIds: number[]): Promise<void> {
    if (state.scope === "selected" && selectedIds.length === 0) {
      state.error = "No books are selected.";
      return;
    }
    if (state.scope === "search" && selectedIds.length === 0) {
      state.error = "No books match the current search.";
      return;
    }
    state.phase = "finding";
    state.error = null;
    state.report = null;
    state.undone = null;
    try {
      const template = state.renameTemplate;
      const res = await api.suggestRetitles(scopeFor(state.scope, selectedIds), template);
      state.rows = orderRows(res.rows.map(rowFromServer));
      state.serverTotal = res.total;
      state.truncated = res.truncated;
      state.templateAtFind = template.trim();
      state.filter = "all";
      state.page = 0;
      state.phase = "ready";
    } catch (e) {
      state.rows = [];
      state.phase = "idle";
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** Applies every ticked row as one job. Refuses with nothing ticked or a ticked row without a title. */
  async function apply(): Promise<RetitleReport | null> {
    if (!canApply()) {
      state.error = checkedCount() === 0 ? "Nothing is ticked." : "Every ticked book needs a title.";
      return null;
    }
    const items = itemsToApply(state.rows);
    state.phase = "running";
    state.error = null;
    state.progress = "Starting…";
    try {
      const report = await api.runRetitle(
        items,
        { embed: state.embed, renameFiles: state.renameFiles, renameTemplate: state.renameTemplate },
        { onStart: (id) => (state.jobId = id), onProgress: (job) => (state.progress = describeJobProgress(job.progress)) },
      );
      state.report = report;
      state.phase = "done";
      state.rows = [];
      await loadRuns();
      return report;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      state.phase = "ready";
      return null;
    } finally {
      state.jobId = null;
      state.progress = "";
    }
  }

  async function loadRuns(): Promise<void> {
    try {
      state.runs = (await api.fetchRetitleRuns()).runs.filter((r) => !r.undone);
    } catch {
      state.runs = [];
    }
  }

  /** Puts a run's titles and file names back. */
  async function undo(runId: string): Promise<boolean> {
    state.error = null;
    state.progress = "Putting titles back…";
    const before = state.phase;
    state.phase = "running";
    try {
      const done = await api.undoRetitle(runId, { onStart: (id) => (state.jobId = id) });
      state.undone = describeUndo(done);
      state.runs = state.runs.filter((r) => r.run_id !== runId);
      if (state.report?.run_id === runId) state.report = null;
      state.phase = "idle";
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      state.phase = before === "running" ? "idle" : before;
      return false;
    } finally {
      state.jobId = null;
      state.progress = "";
    }
  }

  return { state, open, setScope, find, apply, undo, loadRuns, pageRows, setFilter, setPage, setAllChecked, checkedCount, proposedIsStale, canApply };
}

export type RetitleController = ReturnType<typeof createRetitleController>;
