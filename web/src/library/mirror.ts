// The "Export a copy…" dialog: a travel copy you can refresh, or a one-way
// backup mirror, written to another folder or drive.
//
// The server is the authority (crates/calibre_srv/src/library_mirror.rs): it
// plans the copy, protects edits made in the copy (a conflict is skipped unless
// overwriting is asked for), moves what it removes to `.mirror-trash` in the
// copy, and refuses to run a plan that was not previewed. This module holds the
// dialog's state machine so its rules can be tested:
//
// * a run needs a preview of exactly this draft (any edit drops it);
// * a backup mirror always holds the whole library; only a travel copy has a scope;
// * "Browse…" is offered only in the desktop app, which has a native folder dialog.

import { reactive } from "vue";
import * as realApi from "./api";
import { tauriInvoke, isTauri } from "../tauri";
import type { LibraryJobProgress, MirrorMode, MirrorPlan, MirrorPlanItem, MirrorReport, MirrorRequest, MirrorScope, MirrorTarget } from "./api";

export type MirrorApi = Pick<typeof realApi, "planMirror" | "runMirror" | "fetchMirrorTargets" | "forgetMirrorTarget">;

export interface MirrorEnv {
  /** Whether the page runs inside the desktop app. */
  isDesktop: () => boolean;
  /** The native folder dialog; the folder's path, or null if cancelled. */
  pickFolder: () => Promise<string | null>;
}

export const realEnv: MirrorEnv = {
  isDesktop: isTauri,
  pickFolder: () => tauriInvoke<string | null>("pick_folder"),
};

export type MirrorScopeKind = "all" | "selected" | "folder";

export interface MirrorDraft {
  mode: MirrorMode;
  target: string;
  /** Travel only. */
  scope: MirrorScopeKind;
  /** Relative to the library, for the "folder" scope. */
  folderPrefix: string;
  removeMissing: boolean;
  overwriteChanged: boolean;
}

export const MODE_LABELS: Record<MirrorMode, { title: string; hint: string }> = {
  travel: { title: "Travel copy: a library you can take with you", hint: "Opens as a library of its own, on a laptop or a USB disk. Refresh it later to bring it up to date." },
  mirror: { title: "Backup mirror: an exact copy for safekeeping, e.g. on the NAS", hint: "A one-way copy of the whole library, kept up to date by running it again. Restore by opening the folder." },
};

/** What a dialog opens with: a travel copy of the selection when there is one, else everything. */
export function defaultDraft(selectedCount = 0): MirrorDraft {
  return { mode: "travel", target: "", scope: selectedCount > 0 ? "selected" : "all", folderPrefix: "", removeMissing: true, overwriteChanged: false };
}

/** Whether to offer the native folder dialog. Only the desktop app has one. */
export function browseVisible(isDesktop: boolean): boolean {
  return isDesktop;
}

/** The scope the server is asked for. A backup mirror is always the whole library. */
export function draftToScope(d: MirrorDraft, selectedIds: number[]): MirrorScope {
  if (d.mode === "mirror" || d.scope === "all") return { all: true };
  if (d.scope === "selected") return { book_ids: [...selectedIds] };
  return { folder_prefix: d.folderPrefix.trim().replace(/^[\\/]+|[\\/]+$/g, "") };
}

/** A path that starts at a drive (`C:\…`), a share (`\\nas\…`) or the root (`/…`). */
export function looksAbsolute(path: string): boolean {
  const p = path.trim();
  return /^[A-Za-z]:[\\/]/.test(p) || /^\\\\[^\\]/.test(p) || p.startsWith("/");
}

/** Why the draft cannot be previewed, or null. The server checks the folder itself (inside the library, not empty…). */
export function draftProblem(d: MirrorDraft, selectedIds: number[]): string | null {
  const target = d.target.trim();
  if (target === "") return "Choose the folder to put the copy in.";
  if (!looksAbsolute(target)) return "Give the whole path of the folder, starting from a drive (D:\\Library) or a share (\\\\nas\\share\\Library).";
  if (/(^|[\\/])\.\.([\\/]|$)/.test(target)) return "The path cannot contain “..”.";
  if (d.mode === "travel" && d.scope === "selected" && selectedIds.length === 0) return "No books are selected.";
  if (d.mode === "travel" && d.scope === "folder" && draftToScope(d, selectedIds).folder_prefix === "") return "Enter the folder whose books go in the copy.";
  return null;
}

export function draftToRequest(d: MirrorDraft, selectedIds: number[]): MirrorRequest {
  return { target: d.target.trim(), mode: d.mode, scope: draftToScope(d, selectedIds), remove_missing: d.removeMissing, overwrite_changed: d.overwriteChanged };
}

/** Identifies a draft, so an edit after the preview can be told from no edit. */
export function planKey(d: MirrorDraft, selectedIds: number[]): string {
  const r = draftToRequest(d, selectedIds);
  const scope = r.scope ?? {};
  return JSON.stringify([r.target, r.mode, scope.all ?? false, scope.book_ids ? [...scope.book_ids].sort((a, b) => a - b) : null, scope.folder_prefix ?? null, r.remove_missing, r.overwrite_changed]);
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (v: number, one: string, many = `${one}s`) => `${n(v)} ${v === 1 ? one : many}`;

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/** The totals line of a preview: new, updated, unchanged, to remove, conflicts, and what would be copied. */
export function describePlan(p: MirrorPlan): string {
  const t = p.totals;
  const parts = [`${n(t.copy_new.files)} new`, `${n(t.update.files)} updated`, `${n(t.unchanged.files)} unchanged`];
  if (t.remove.files > 0) parts.push(`${n(t.remove.files)} to remove`);
  if (t.conflict.files > 0) parts.push(`${plural(t.conflict.files, "conflict")}`);
  parts.push(t.bytes_to_copy > 0 ? `${formatBytes(t.bytes_to_copy)} to copy` : "nothing to copy");
  return parts.join(" · ");
}

/** True when a listed section is shorter than its total (the server caps each list). */
export function listIsPartial(p: MirrorPlan, action: keyof MirrorPlan["items"]): boolean {
  return p.truncated[action] || p.items[action].length < p.totals[action].files;
}

/** The copy's database files: a conflict on one of them means edits made in the copy. */
const DATABASE_PATHS = ["metadata.db"];

/** A conflict in plain words. The one that matters most is the travel copy's own database. */
export function conflictText(item: MirrorPlanItem): string {
  if (DATABASE_PATHS.includes(item.path)) {
    return "metadata.db in the copy was changed since the last refresh, probably edited on the other computer. Refreshing would discard those edits.";
  }
  return item.reason ? `${item.path}: ${item.reason}.` : `${item.path} in the copy was changed since the last refresh.`;
}

/** Said when the plan lists a conflict, once: what it means and what to do. */
export function conflictsNote(p: MirrorPlan): string | null {
  if (p.totals.conflict.files === 0) return null;
  const edits = p.copy_edits.count > 0 ? ` The copy has ${plural(p.copy_edits.count, "change")} of its own.` : "";
  return `${plural(p.totals.conflict.files, "file")} in the copy changed since the last refresh and will be skipped, so nothing there is lost.${edits} To replace them with the library’s version, tick “Overwrite files changed in the copy”.`;
}

/** On a network folder the copy goes file by file and no database is ever opened there. */
export function networkNote(p: MirrorPlan): string | null {
  return p.network ? "This folder is on the network. The copy is written file by file, and the database is never opened over the network." : null;
}

export function describeReport(r: MirrorReport): string {
  const parts = [`${plural(r.copied, "file")} copied`, `${n(r.updated)} updated`];
  if (r.removed > 0) parts.push(`${n(r.removed)} moved to .mirror-trash`);
  parts.push(`${formatBytes(r.bytes_copied)} written`);
  if (r.conflicts.length > 0) parts.push(`${plural(r.conflicts.length, "conflict")} skipped`);
  if (r.failures.length > 0) parts.push(`${plural(r.failures.length, "file")} could not be copied`);
  if (r.cancelled) parts.push("stopped early — run it again to continue");
  return parts.join(" · ");
}

/** One line for the running job. `added` is files written, `bytes_hashed` bytes, `total` files to write. */
export function describeProgress(p: LibraryJobProgress): string {
  switch (p.phase) {
    case "mirror_files":
      return `Copying… ${n(p.added + p.errors)} of ${n(p.total)} files · ${formatBytes(p.bytes_hashed)}`;
    case "mirror_databases":
      return `Writing the databases… ${n(p.added + p.errors)} of ${n(p.total)} files`;
    case "mirror_removing":
      return "Setting aside files the library no longer has…";
    case "mirror_manifest":
      return "Finishing…";
    default:
      return "Starting…";
  }
}

export function describeScope(s: MirrorTarget["scope"]): string {
  if (s === "all" || s === null) return "the whole library";
  if ("book_ids" in s) return plural(s.book_ids.length, "selected book");
  if ("folder_prefix" in s) return `the folder ${s.folder_prefix}`;
  return `${n(s.too_large)} selected books`;
}

/** A remembered target in one line: when it last ran and how it went. */
export function describeTarget(t: MirrorTarget): string {
  const r = t.last_result;
  const when = t.last_run_at ? `last run ${new Date(t.last_run_at * 1000).toLocaleString()}` : "never run";
  if (!r) return when;
  if (r.error) return `${when} · failed: ${r.error}`;
  const bits = [`${n(r.copied ?? 0)} new`, `${n(r.updated ?? 0)} updated`];
  if ((r.removed ?? 0) > 0) bits.push(`${n(r.removed ?? 0)} removed`);
  if ((r.conflicts ?? 0) > 0) bits.push(`${plural(r.conflicts ?? 0, "conflict")}`);
  if ((r.failures ?? 0) > 0) bits.push(`${n(r.failures ?? 0)} failed`);
  if (r.cancelled) bits.push("stopped early");
  return `${when} · ${bits.join(", ")}`;
}

export function reachableLabel(t: MirrorTarget): string {
  return t.reachable ? "reachable" : "not connected";
}

/** The scope a remembered target had, as the draft's scope choice (a selection that was not stored cannot be reused). */
export function targetScopeKind(t: MirrorTarget): { scope: MirrorScopeKind; folderPrefix: string; needsSelection: boolean } {
  const s = t.scope;
  if (s === "all" || s === null) return { scope: "all", folderPrefix: "", needsSelection: false };
  if ("folder_prefix" in s) return { scope: "folder", folderPrefix: s.folder_prefix, needsSelection: false };
  if ("book_ids" in s) return { scope: "selected", folderPrefix: "", needsSelection: false };
  return { scope: "selected", folderPrefix: "", needsSelection: true };
}

export type MirrorPhase = "idle" | "planning" | "planned" | "running" | "done";

export function createMirrorController(api: MirrorApi = realApi, env: MirrorEnv = realEnv) {
  const state = reactive({
    draft: defaultDraft(),
    phase: "idle" as MirrorPhase,
    plan: null as MirrorPlan | null,
    /** The draft the plan was made for. */
    planKey: "",
    report: null as MirrorReport | null,
    progress: "",
    jobId: null as number | null,
    error: null as string | null,
    targets: [] as MirrorTarget[],
    /** The ids a remembered "selected books" scope held, used by the refresh that follows. */
    rememberedIds: null as number[] | null,
    notice: null as string | null,
  });

  const canBrowse = () => browseVisible(env.isDesktop());

  /** Opens fresh: the scope follows the selection, nothing previewed. */
  function open(selectedCount: number): void {
    state.draft = defaultDraft(selectedCount);
    state.phase = "idle";
    state.plan = null;
    state.planKey = "";
    state.report = null;
    state.error = null;
    state.progress = "";
    state.jobId = null;
    state.rememberedIds = null;
    state.notice = null;
  }

  function planIsCurrent(selectedIds: number[]): boolean {
    return state.plan !== null && state.planKey === planKey(state.draft, effectiveIds(selectedIds));
  }

  /** A refresh of a remembered selection uses the books it held, not today's selection. */
  function effectiveIds(selectedIds: number[]): number[] {
    return state.rememberedIds ?? selectedIds;
  }

  /** An edit invalidates the preview (the run button needs one for exactly this draft). */
  function edited(): void {
    if (state.phase === "planned") {
      state.plan = null;
      state.phase = "idle";
    }
    // Choosing another scope means the remembered selection no longer applies.
    if (state.draft.scope !== "selected") state.rememberedIds = null;
  }

  async function loadTargets(): Promise<void> {
    try {
      state.targets = await api.fetchMirrorTargets();
    } catch {
      state.targets = [];
    }
  }

  async function forget(target: string): Promise<void> {
    try {
      state.targets = await api.forgetMirrorTarget(target);
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** The native folder dialog, desktop only. Fills the target when a folder is picked. */
  async function browse(): Promise<void> {
    if (!canBrowse()) return;
    try {
      const picked = await env.pickFolder();
      if (picked) {
        state.draft.target = picked;
        edited();
      }
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  async function preview(selectedIds: number[]): Promise<void> {
    const ids = effectiveIds(selectedIds);
    const problem = draftProblem(state.draft, ids);
    if (problem) {
      state.error = problem;
      return;
    }
    const key = planKey(state.draft, ids);
    state.phase = "planning";
    state.error = null;
    state.report = null;
    try {
      state.plan = await api.planMirror(draftToRequest(state.draft, ids));
      state.planKey = key;
      state.phase = "planned";
    } catch (e) {
      state.plan = null;
      state.phase = "idle";
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** Prefills the draft from a remembered target and previews it. */
  /** Overwriting the copy's own edits is a one-off decision: a refresh always
   *  starts with it off, whatever the last run used. */
  async function refresh(target: MirrorTarget, selectedIds: number[]): Promise<void> {
    const scope = targetScopeKind(target);
    state.rememberedIds = null;
    state.notice = null;
    state.draft = { mode: target.mode, target: target.target, scope: scope.scope, folderPrefix: scope.folderPrefix, removeMissing: target.remove_missing, overwriteChanged: false };
    if (target.scope && typeof target.scope === "object" && "book_ids" in target.scope) state.rememberedIds = [...target.scope.book_ids];
    if (scope.needsSelection) {
      state.plan = null;
      state.phase = "idle";
      state.error = null;
      state.notice = "This copy held a large selection that was not remembered. Select the books again, or choose another scope.";
      return;
    }
    await preview(selectedIds);
  }

  /** Runs the previewed plan. Refuses without a preview of the current draft. */
  async function run(selectedIds: number[]): Promise<MirrorReport | null> {
    const plan = state.plan;
    const ids = effectiveIds(selectedIds);
    if (!plan || state.phase !== "planned" || !planIsCurrent(selectedIds)) {
      state.error = "Preview first.";
      return null;
    }
    state.phase = "running";
    state.error = null;
    state.progress = "Starting…";
    try {
      const report = await api.runMirror(draftToRequest(state.draft, ids), plan.fingerprint, {
        onStart: (id) => (state.jobId = id),
        onProgress: (job) => (state.progress = describeProgress(job.progress)),
      });
      state.report = report;
      state.phase = "done";
      state.plan = null;
      await loadTargets();
      return report;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      // A 409 means the library or the copy moved on: the preview is void.
      state.plan = null;
      state.phase = "idle";
      return null;
    } finally {
      state.jobId = null;
      state.progress = "";
    }
  }

  return { state, open, planIsCurrent, edited, preview, run, refresh, loadTargets, forget, browse, canBrowse };
}

export type MirrorController = ReturnType<typeof createMirrorController>;
