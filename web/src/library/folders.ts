// The "Folders" category of the category browser: the library's on-disk folder
// tree, filtering by a folder, and moving books or renaming a folder.
//
// The server (crates/lexmechanic_srv/src/library_folders.rs) lists the folders with
// counts that include sub-folders and plans every move; this module holds what
// is worth testing without a browser: building the tree from the flat list,
// which rows are visible, the `folder:` search a click makes, the drag payload,
// and the plan -> confirm -> run -> undo flow shared by "Move selected books
// here", drag and drop, and "Rename folder".

import { reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { LibraryFolder, OrganizePlan, OrganizeReport, OrganizeUndoReport } from "./api";

export const FOLDERS_EXPANDED_KEY = "lexmechanic.folders.expanded";
/** The drag data type carrying book ids from the table to a folder. */
export const BOOKS_DRAG_TYPE = "application/x-lexmechanic-books";

export interface FolderNode {
  /** Full path, `/`-separated. */
  path: string;
  /** Last segment. */
  name: string;
  /** Books here and below. */
  books: number;
  booksHere: number;
  empty: boolean;
  children: FolderNode[];
}

/** Nests the server's flat, sorted list. A missing parent (never sent by the server) is made up. */
export function buildFolderTree(folders: LibraryFolder[]): FolderNode[] {
  const roots: FolderNode[] = [];
  const byPath = new Map<string, FolderNode>();
  const node = (path: string, from?: LibraryFolder): FolderNode => {
    const key = path.toLowerCase();
    let n = byPath.get(key);
    if (!n) {
      n = { path, name: path.slice(path.lastIndexOf("/") + 1), books: 0, booksHere: 0, empty: false, children: [] };
      byPath.set(key, n);
      const cut = path.lastIndexOf("/");
      (cut < 0 ? roots : node(path.slice(0, cut)).children).push(n);
    }
    if (from) Object.assign(n, { books: from.books, booksHere: from.books_here, empty: from.empty });
    return n;
  };
  for (const f of folders) node(f.path, f);
  return roots;
}

export interface FolderRow {
  node: FolderNode;
  depth: number;
}

/** The rows drawn: a node's children only when it is expanded. */
export function visibleFolderRows(roots: FolderNode[], expanded: ReadonlySet<string>): FolderRow[] {
  const rows: FolderRow[] = [];
  const walk = (nodes: FolderNode[], depth: number) => {
    for (const n of nodes) {
      rows.push({ node: n, depth });
      if (n.children.length > 0 && expanded.has(n.path)) walk(n.children, depth + 1);
    }
  };
  walk(roots, 0);
  return rows;
}

/** The ancestors of a folder, so revealing it can expand the way down. */
export function ancestorsOf(path: string): string[] {
  const parts = path.split("/");
  return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
}

export function toggleFolder(expanded: ReadonlySet<string>, path: string): Set<string> {
  const next = new Set(expanded);
  if (!next.delete(path)) next.add(path);
  return next;
}

export function parseExpandedFolders(raw: string | null): Set<string> {
  try {
    const parsed = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : []);
  } catch {
    return new Set();
  }
}

/**
 * The search a click on a folder makes: the folder and everything below it, by
 * whole path segment (`Rift` is not `Rifts`). Quotes and backslashes in the name
 * are escaped for the search language.
 */
export function folderQuery(path: string): string {
  const clean = path.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  return `folder:"${clean.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Why a name cannot be a single folder name, or null. */
export function folderNameProblem(name: string): string | null {
  const n = name.trim();
  if (!n) return "Enter a name.";
  if (/[\\/]/.test(n)) return "A folder name cannot contain / or \\.";
  if (n === "." || n === "..") return "That is not a folder name.";
  if (/[<>:"|?*\u0000-\u001f]/.test(n)) return "A folder name cannot contain < > : \" | ? * .";
  if (/[. ]$/.test(n)) return "A folder name cannot end with a dot or a space.";
  return null;
}

export function joinFolder(parent: string, name: string): string {
  return parent ? `${parent}/${name.trim()}` : name.trim();
}

/** Where a folder would be after renaming its last segment. */
export function renamedPath(path: string, newName: string): string {
  const cut = path.lastIndexOf("/");
  return joinFolder(cut < 0 ? "" : path.slice(0, cut), newName);
}

// ---- Drag and drop ----

interface DragData {
  types?: readonly string[];
  setData(type: string, value: string): void;
  getData(type: string): string;
  effectAllowed?: string;
}

/** The ids a drag from the table carries: the selection when the dragged row is in it, else just that row. */
export function dragIds(rowId: number, selected: ReadonlySet<number>, selectMode: boolean): number[] {
  return selectMode && selected.has(rowId) ? [...selected] : [rowId];
}

export function setBooksDrag(data: DragData, ids: number[]): void {
  data.setData(BOOKS_DRAG_TYPE, JSON.stringify(ids));
  data.setData("text/plain", `${ids.length} book${ids.length === 1 ? "" : "s"}`);
  data.effectAllowed = "move";
}

export function hasBooksDrag(data: { types?: readonly string[] }): boolean {
  return Array.from(data.types ?? []).includes(BOOKS_DRAG_TYPE);
}

export function readBooksDrag(data: DragData): number[] {
  try {
    const parsed = JSON.parse(data.getData(BOOKS_DRAG_TYPE));
    return Array.isArray(parsed) ? parsed.filter((n): n is number => Number.isInteger(n)) : [];
  } catch {
    return [];
  }
}

// ---- The move / rename flow ----

export type FolderApi = Pick<typeof realApi, "planMoveBooks" | "runMoveBooks" | "planRenameFolder" | "runRenameFolder" | "undoOrganize">;

export type FolderAction = { kind: "move"; ids: number[]; folder: string } | { kind: "rename"; from: string; to: string };
export type FolderFlowPhase = "idle" | "planning" | "confirm" | "running" | "done";

/** A move of fewer books than this, with no name collisions, runs without a preview. */
export const QUICK_MOVE_BOOKS = 20;

/** Whether a planned move is small and clean enough to do at once (it is still undoable). */
export function isQuickMove(p: Pick<OrganizePlan, "books" | "conflicts" | "skipped">): boolean {
  return p.books > 0 && p.books < QUICK_MOVE_BOOKS && p.conflicts === 0 && p.skipped.length === 0;
}

/** The toast after a quick move. */
export function describeQuickMove(r: Pick<OrganizeReport, "books_moved">, folder: string): string {
  return `Moved ${plural(r.books_moved, "book")} to ${folder}`;
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (v: number, one: string) => `${n(v)} ${one}${v === 1 ? "" : "s"}`;

/** The summary line the confirm step shows. */
export function describeFolderPlan(action: FolderAction, p: OrganizePlan): string {
  const parts = [`${plural(p.books, "book")} (${plural(p.files, "file")}) would move`];
  if (p.already_organized > 0) parts.push(`${n(p.already_organized)} already there`);
  if (p.conflicts > 0) parts.push(`${plural(p.conflicts, "name")} taken, so numbered`);
  if (p.skipped.length > 0) parts.push(`${n(p.skipped.length)} skipped`);
  return `${action.kind === "move" ? `To ${action.folder}` : `${action.from} → ${action.to}`}: ${parts.join(" · ")}`;
}

export function describeFolderReport(r: OrganizeReport): string {
  const parts = [`${plural(r.books_moved, "book")} moved`];
  if (r.failed.length > 0) parts.push(`${n(r.failed.length)} could not be moved and ${r.failed.length === 1 ? "was" : "were"} left where ${r.failed.length === 1 ? "it" : "they"} were`);
  return parts.join(" · ");
}

export function describeFolderUndo(r: OrganizeUndoReport): string {
  return `${plural(r.books_restored, "book")} put back${r.skipped.length > 0 ? ` · ${n(r.skipped.length)} left alone because they changed since` : ""}`;
}

/**
 * Dry run -> confirm -> run -> (undo). The run is refused by the server unless
 * it is the plan that was previewed (the fingerprint), and `onChanged` fires
 * after a run or an undo so the list can refresh in place.
 */
export function createFolderFlow(api: FolderApi = realApi, onChanged: () => void | Promise<void> = () => {}) {
  const state = reactive({
    phase: "idle" as FolderFlowPhase,
    action: null as FolderAction | null,
    plan: null as OrganizePlan | null,
    report: null as OrganizeReport | null,
    progress: "",
    error: null as string | null,
    undone: null as string | null,
    /** A small move is being planned or run without its dialog; see `begin`. */
    quiet: false,
    /** The last run was a quick move, so it is reported in a toast. */
    quick: false,
  });

  function reset(): void {
    state.quiet = false;
    state.quick = false;
    state.phase = "idle";
    state.action = null;
    state.plan = null;
    state.report = null;
    state.progress = "";
    state.error = null;
    state.undone = null;
  }

  /**
   * Starts a flow and previews it. Ends in `confirm`, or back in `idle` with an error shown.
   *
   * With `quick`, a small, clean move (see [`isQuickMove`]) is not previewed at
   * all: it runs at once and ends in `done` with `state.quick` set, and the
   * caller says so in a toast with an Undo. A move that is large or has
   * collisions falls back to the preview, as does any failure.
   */
  async function begin(action: FolderAction, options: { quick?: boolean } = {}): Promise<boolean> {
    reset();
    state.action = action;
    state.phase = "planning";
    state.quiet = !!options.quick && action.kind === "move" && action.ids.length < QUICK_MOVE_BOOKS;
    try {
      state.plan = action.kind === "move" ? await api.planMoveBooks(action.ids, action.folder) : await api.planRenameFolder(action.from, action.to);
      state.phase = state.plan.books === 0 ? "idle" : "confirm";
      if (state.plan.books === 0) state.error = state.plan.skipped[0]?.reason ?? "Nothing to move: the books are already there.";
      if (state.quiet && state.plan.books > 0 && isQuickMove(state.plan)) {
        state.quick = true;
        const ran = await run();
        // A failure, or books that could not be moved, is for the dialog to show.
        state.quiet = false;
        if (!ran || (state.report?.failed.length ?? 0) > 0) state.quick = false;
        return ran;
      }
      state.quiet = false;
      return state.plan.books > 0;
    } catch (e) {
      state.plan = null;
      state.phase = "idle";
      state.quiet = false;
      state.error = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  /** Runs the previewed plan. */
  async function run(): Promise<boolean> {
    const { action, plan } = state;
    if (!action || !plan || state.phase !== "confirm") return false;
    state.phase = "running";
    state.error = null;
    state.progress = "Starting…";
    const handlers = { onProgress: (job: { progress: Parameters<typeof describeJobProgress>[0] }) => (state.progress = describeJobProgress(job.progress)) };
    try {
      state.report = action.kind === "move" ? await api.runMoveBooks(action.ids, action.folder, plan.plan_fingerprint, handlers) : await api.runRenameFolder(action.from, action.to, plan.plan_fingerprint, handlers);
      state.phase = "done";
      await onChanged();
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      // A 409 means the library moved on: the preview is void.
      state.plan = null;
      state.phase = "idle";
      return false;
    } finally {
      state.progress = "";
    }
  }

  /** Puts the last run back. */
  async function undo(): Promise<boolean> {
    const runId = state.report?.run_id;
    if (!runId) return false;
    state.phase = "running";
    state.progress = "Putting files back…";
    try {
      const done = await api.undoOrganize(runId);
      state.undone = describeFolderUndo(done);
      state.report = null;
      state.phase = "idle";
      await onChanged();
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      state.phase = "done";
      return false;
    } finally {
      state.progress = "";
    }
  }

  return { state, begin, run, undo, reset };
}

export type FolderFlow = ReturnType<typeof createFolderFlow>;
