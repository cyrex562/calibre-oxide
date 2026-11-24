// The compare view: 2 to 8 copies of a book side by side, to look at, edit,
// move and decide between (tranche 16).
//
// The server does the facts (crates/lexmechanic_srv/src/duplicates_compare.rs:
// every book's files and PDF facts, and per pair whether the bytes are
// identical and what the cached page fingerprints say). Everything else the
// view does goes through routes that already existed: `setFields` for the
// title and authors, `renameFiles` (with a literal name) for the file name,
// `/move-books` for the folder, the duplicates routes for groups, "not
// duplicates" and the resolve run. This module holds the view's state and
// rules so they can be tested without a browser; CompareDuplicates.vue is the
// template over it.
//
// The rules worth knowing:
//
// * The page control is shared. A cursor is an anchor (start, middle or end)
//   and an offset from it, so "Last" is each file's own last page, "Middle"
//   its own middle, and "align from the end" counts page n from the end.
// * The pair evidence is highlighted when the pages on screen are a pair of
//   sample pages the server compared.
// * Deciding goes through the resolve route for exactly the set on screen:
//   the set is made a group of the user's own first (unless it already is
//   one), and nothing is trashed unless that group holds exactly these books.

import { reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { BookFieldChanges } from "./types";
import type { CompareBook, ComparePair, CompareResult, DuplicateGroup, ManualGroupResult, ResolvePlan, ResolveSelection } from "./api";
import { confidencePercent, formatBytes, formatMtime, reasonLabel } from "./duplicates";

export type CompareApi = Pick<
  typeof realApi,
  | "fetchCompare"
  | "setFields"
  | "renameFiles"
  | "planMoveBooks"
  | "runMoveBooks"
  | "undoOrganize"
  | "refreshDuplicateBooks"
  | "createManualGroup"
  | "addToManualGroup"
  | "removeFromManualGroup"
  | "fetchManualGroups"
  | "deleteManualGroup"
  | "markNotDuplicates"
  | "planResolve"
  | "runResolve"
  | "restoreResolveRun"
  | "runDuplicateScan"
  | "search"
  | "fetchBooks"
>;

/** What the view tells the rest of the app. */
export interface CompareHooks {
  /** A book's title, file name or folder changed: the library list is out of date. */
  libraryChanged?: () => void;
  /** The review list's groups changed (a group made, resolved, dismissed): reload it. */
  groupsChanged?: () => void | Promise<void>;
}

export const MIN_BOOKS = 2;
export const MAX_BOOKS = 8;
/** CSS pixels a PDF page is drawn at in each column. */
export const PREVIEW_WIDTH = 420;

// ---------------------------------------------------------------------------
// The shared page cursor
// ---------------------------------------------------------------------------

export type PageAnchor = "start" | "middle" | "end";

/** Where every column is looking: an anchor in each file, and pages away from it. */
export interface PageCursor {
  anchor: PageAnchor;
  /** Pages from the anchor: toward the end of the file for `start` and `middle`, toward the start for `end`. */
  offset: number;
}

export const FIRST: PageCursor = { anchor: "start", offset: 0 };
export const MIDDLE: PageCursor = { anchor: "middle", offset: 0 };
export const LAST: PageCursor = { anchor: "end", offset: 0 };

/** The 1-based page a file of `pageCount` pages shows for `cursor`, kept inside the file. */
export function pageAt(cursor: PageCursor, pageCount: number): number {
  if (pageCount < 1) return 1;
  const raw = cursor.anchor === "start" ? 1 + cursor.offset : cursor.anchor === "end" ? pageCount - cursor.offset : Math.ceil(pageCount / 2) + cursor.offset;
  return Math.min(pageCount, Math.max(1, raw));
}

/** One page forward (`+1`) or back (`-1`), never past the longest file's ends. */
export function stepCursor(cursor: PageCursor, delta: 1 | -1, longest: number): PageCursor {
  const move = cursor.anchor === "end" ? -delta : delta;
  let offset = cursor.offset + move;
  if (cursor.anchor === "middle") offset = Math.min(longest, Math.max(-longest, offset));
  else offset = Math.min(Math.max(0, longest - 1), Math.max(0, offset));
  return { anchor: cursor.anchor, offset };
}

/** "Go to page n": counted from the end when `fromEnd`. */
export function cursorForNumber(n: number, fromEnd: boolean, longest: number): PageCursor {
  const offset = Math.min(Math.max(0, longest - 1), Math.max(0, Math.floor(Number.isFinite(n) ? n : 1) - 1));
  return { anchor: fromEnd ? "end" : "start", offset };
}

/**
 * Switches between counting from the start and from the end without moving:
 * the reference file (`pageCount`) keeps the page it is showing.
 */
export function realign(cursor: PageCursor, fromEnd: boolean, pageCount: number): PageCursor {
  const page = pageAt(cursor, pageCount);
  return fromEnd ? { anchor: "end", offset: pageCount - page } : { anchor: "start", offset: page - 1 };
}

/** The number in the page box: the reference page, or its place from the end. */
export function cursorNumber(cursor: PageCursor, pageCount: number): number {
  return cursor.anchor === "end" ? cursor.offset + 1 : pageAt(cursor, pageCount);
}

// ---------------------------------------------------------------------------
// What a pair says
// ---------------------------------------------------------------------------

/**
 * Whether the pages on screen are a pair the server compared: `matched` when
 * they are sample pages that match, `different` when they were compared and do
 * not, null when they are not sample pages (or there is no comparison).
 */
export function pairPageStatus(pair: ComparePair, pageA: number | null, pageB: number | null): "matched" | "different" | null {
  if (pageA === null || pageB === null || !pair.pdf?.pages) return null;
  const hit = pair.pdf.pages.find((p) => p.a === pageA - 1 && p.b === pageB - 1);
  return hit ? (hit.matched ? "matched" : "different") : null;
}

/** "93%" / "0.93" for a text similarity. */
function similarity(j: number): string {
  return j.toFixed(2);
}

/** The one-line verdict of a pair's page comparison. */
export function pdfLine(pair: ComparePair): string {
  const p = pair.pdf;
  if (!p) return "";
  if (!p.fingerprinted) return "Pages not analysed yet: use “Analyse library” in Find duplicates to compare them.";
  const parts: string[] = [];
  if (p.kind && p.confidence != null) parts.push(`${confidencePercent(p.confidence)} · ${reasonLabel(p.kind)}`);
  else parts.push("The pages do not match");
  if (p.compared) parts.push(`${p.matched ?? 0} of ${p.compared} sample pages match`);
  if (p.text_jaccard != null) parts.push(`text similarity ${similarity(p.text_jaccard)}`);
  return parts.join(" · ");
}

/** Every line of evidence about a pair, strongest first: bytes, pages, title and author. */
export function pairLines(pair: ComparePair): string[] {
  const lines: string[] = [];
  if (pair.identical_bytes === true) lines.push(`Identical files (${pair.identical_formats.join(", ") || "same bytes"})`);
  else if (pair.identical_bytes === false) lines.push("The files differ");
  // "Pages not analysed yet" is said once for the whole view (the evidence block's note), not under every pair.
  const pdf = pair.pdf && !pair.pdf.fingerprinted ? "" : pdfLine(pair);
  if (pdf) lines.push(pdf);
  for (const e of pair.evidence) lines.push(`${reasonLabel(e.reason)} (${confidencePercent(e.confidence)}): ${e.detail}`);
  if (pair.exempt) lines.push("You marked these as not duplicates");
  return lines;
}

// ---------------------------------------------------------------------------
// The key facts, one row per fact and one column per copy
// ---------------------------------------------------------------------------

/** What a PDF's text layer is, in a word. */
export function textLayerOf(b: Pick<CompareBook, "pdf">): string {
  if (!b.pdf) return "";
  if (b.pdf.has_text) return "text layer";
  return b.pdf.image_only || b.pdf.scanned ? "image only" : "no text";
}

/** The file name (with extension) of a book's first file. */
export function fileNameOf(b: Pick<CompareBook, "files">): string {
  return (b.files?.[0]?.path ?? "").replace(/\\/g, "/").split("/").pop() ?? "";
}

export interface FactRow {
  key: string;
  label: string;
  /** One value per book, in the books' order; "—" where there is none. */
  values: string[];
  /** The copies do not all agree: the row is highlighted. */
  differs: boolean;
}

/**
 * The compact table the view opens with: the facts that tell copies apart,
 * lined up so a difference is one glance. Rows every copy agrees on are marked
 * so they can be quieted; the size is shown once (not once for the book and
 * once for the file).
 */
export function factRows(books: readonly CompareBook[]): FactRow[] {
  const dash = (v: string) => (v.trim() === "" ? "—" : v);
  const row = (key: string, label: string, get: (b: CompareBook) => string): FactRow => {
    const values = books.map((b) => dash(get(b)));
    return { key, label, values, differs: new Set(values).size > 1 };
  };
  const rows: FactRow[] = [
    row("title", "Title", (b) => b.title),
    row("authors", "Authors", (b) => b.authors.join(" & ")),
    row("file", "File", fileNameOf),
    row("folder", "Folder", (b) => b.path ?? folderOf(b)),
    row("size", "Size", (b) => (b.size === undefined ? "" : formatBytes(b.size))),
    row("formats", "Formats", (b) => (b.format_list ?? []).join(", ")),
  ];
  if (books.some((b) => b.pdf)) {
    rows.push(row("pages", "Pages", (b) => (b.pdf ? String(b.pdf.pages) : "")));
    rows.push(row("text", "Text", textLayerOf));
  }
  rows.push(row("modified", "Modified", (b) => formatMtime(b.mtime)));
  return rows;
}

// ---------------------------------------------------------------------------
// Drafts: what the person is typing into each column
// ---------------------------------------------------------------------------

export type DraftField = "title" | "authors" | "stem" | "folder";

export interface Draft {
  title: string;
  authors: string;
  stem: string;
  folder: string;
  /** What the server has, to tell an edit from no edit. */
  base: { title: string; authors: string; stem: string; folder: string };
  errors: Partial<Record<DraftField, string>>;
  saving: Partial<Record<DraftField, boolean>>;
  saved: Partial<Record<DraftField, boolean>>;
  /** The move that can be undone. */
  move: { runId: string; from: string; to: string } | null;
  /** What the move is doing, or did. */
  moveNote: string;
}

/** "A & B" for the authors box. */
export function joinAuthors(authors: string[]): string {
  return authors.join(" & ");
}

export function splitAuthors(text: string): string[] {
  return text.split("&").map((a) => a.trim()).filter(Boolean);
}

/** A file name without its folder or extension. */
export function stemOf(path: string): string {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/** The folder a book is in, relative to the library. */
export function folderOf(book: Pick<CompareBook, "path" | "files">): string {
  if (book.path !== undefined) return book.path;
  const first = book.files?.[0]?.path ?? "";
  const cut = first.replace(/\\/g, "/").lastIndexOf("/");
  return cut > 0 ? first.slice(0, cut) : "";
}

function draftFor(book: CompareBook): Draft {
  const base = { title: book.title, authors: joinAuthors(book.authors), stem: stemOf(book.files?.[0]?.path ?? ""), folder: folderOf(book) };
  return { ...base, base, errors: {}, saving: {}, saved: {}, move: null, moveNote: "" };
}

/** Whether a field differs from what the server has. */
export function isDirty(d: Draft, field: DraftField): boolean {
  return d[field].trim() !== d.base[field].trim();
}

/** The names a file's stem may not contain: the rename route reads `{` as a template. */
export function stemProblem(stem: string): string | null {
  const s = stem.trim();
  if (!s) return "Enter a file name.";
  if (/[{}]/.test(s)) return "A file name cannot contain { or }.";
  if (/[<>:"/\\|?*]/.test(s)) return "A file name cannot contain < > : \" / \\ | ? *";
  return null;
}

function sameSet(a: number[], b: number[]): boolean {
  return a.length === b.length && [...a].sort((x, y) => x - y).every((v, i) => v === [...b].sort((x, y) => x - y)[i]);
}

const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export interface SearchHit {
  id: number;
  title: string;
  authors: string[];
}

export type DecisionPhase = "idle" | "working" | "done" | "error";

// ---------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------

export interface OpenOptions {
  /** The review list's group the view was opened from. */
  group?: Pick<DuplicateGroup, "id" | "manual_group_id"> & { books: { book_id: number }[] };
}

export function createCompareController(api: CompareApi = realApi, hooks: CompareHooks = {}) {
  const state = reactive({
    open: false,
    loading: false,
    error: null as string | null,
    bookIds: [] as number[],
    result: null as CompareResult | null,
    /** The copy to keep: the server's suggestion until the person picks. */
    keeperId: null as number | null,
    keeperChosen: false,
    cursor: { ...FIRST } as PageCursor,
    drafts: {} as Record<number, Draft>,
    /** The review list's group this set is, when it is exactly one. */
    groupId: null as string | null,
    groupBookIds: [] as number[],
    /** The user's own group this set belongs to. */
    manualGroupId: null as string | null,
    groupBusy: false,
    note: null as string | null,
    search: { query: "", busy: false, results: [] as SearchHit[], error: null as string | null },
    decision: { phase: "idle" as DecisionPhase, message: "", runId: null as string | null, trashedIds: [] as number[], progress: "", jobId: null as number | null },
  });

  // ---- loading ----

  /** The reference book for the page control: the first with a PDF. */
  function pagesOf(id: number): number | null {
    return state.result?.books.find((b) => b.book_id === id)?.pdf?.pages ?? null;
  }

  function longestPdf(): number {
    return Math.max(1, ...(state.result?.books ?? []).map((b) => b.pdf?.pages ?? 0));
  }

  function referenceBook(): CompareBook | null {
    return state.result?.books.find((b) => (b.pdf?.pages ?? 0) > 0) ?? null;
  }

  /** The 1-based page a column shows now; null for a book with no PDF. */
  function pageFor(id: number): number | null {
    const pages = pagesOf(id);
    return pages ? pageAt(state.cursor, pages) : null;
  }

  function syncDrafts(result: CompareResult): void {
    const next: Record<number, Draft> = {};
    for (const b of result.books) {
      const fresh = draftFor(b);
      const old = state.drafts[b.book_id];
      if (!old) {
        next[b.book_id] = fresh;
        continue;
      }
      // Keep what the person is typing; take the server's word for the rest.
      const kept = { ...fresh, errors: old.errors, move: old.move, moveNote: old.moveNote };
      for (const f of ["title", "authors", "stem", "folder"] as DraftField[]) {
        if (isDirty(old, f) && old.base[f] === fresh.base[f]) kept[f] = old[f];
      }
      next[b.book_id] = kept;
    }
    state.drafts = next;
  }

  /** Reads the set from the library again. */
  async function load(): Promise<void> {
    if (state.bookIds.length < MIN_BOOKS || state.bookIds.length > MAX_BOOKS) {
      state.error = `Compare takes ${MIN_BOOKS} to ${MAX_BOOKS} books (there are ${state.bookIds.length}).`;
      state.result = null;
      return;
    }
    state.loading = true;
    state.error = null;
    try {
      const result = await api.fetchCompare(state.bookIds);
      state.result = result;
      syncDrafts(result);
      // The person's pick stands while that book is in the set; otherwise the server's suggestion.
      if (!(state.keeperChosen && state.keeperId !== null && result.ids.includes(state.keeperId))) {
        state.keeperId = result.suggested_keeper;
        state.keeperChosen = false;
      }
    } catch (e) {
      state.error = message(e);
    } finally {
      state.loading = false;
    }
  }

  /** Opens the view on these books (2 to 8), from a group card or a selection. */
  async function open(bookIds: number[], options: OpenOptions = {}): Promise<void> {
    state.open = true;
    state.bookIds = [...new Set(bookIds)];
    state.result = null;
    state.drafts = {};
    state.keeperId = null;
    state.keeperChosen = false;
    state.cursor = { ...FIRST };
    state.note = null;
    state.search = { query: "", busy: false, results: [], error: null };
    state.decision = { phase: "idle", message: "", runId: null, trashedIds: [], progress: "", jobId: null };
    const group = options.group;
    state.groupId = group?.id ?? null;
    state.groupBookIds = group ? group.books.map((b) => b.book_id) : [];
    state.manualGroupId = group?.manual_group_id ?? null;
    await load();
  }

  function close(): void {
    state.open = false;
  }

  // ---- the page control ----

  function show(cursor: PageCursor): void {
    state.cursor = cursor;
  }
  const first = () => show({ ...FIRST });
  const middle = () => show({ ...MIDDLE });
  const last = () => show({ ...LAST });
  const step = (delta: 1 | -1) => show(stepCursor(state.cursor, delta, longestPdf()));

  function goTo(n: number): void {
    show(cursorForNumber(n, state.cursor.anchor === "end", longestPdf()));
  }

  /** "Align from the end": page n from the end in every file. */
  function setAlignFromEnd(on: boolean): void {
    const ref = referenceBook();
    if (!ref?.pdf) {
      show(on ? { ...LAST } : { ...FIRST });
      return;
    }
    show(realign(state.cursor, on, ref.pdf.pages));
  }

  function pageNumber(): number {
    const ref = referenceBook();
    return ref?.pdf ? cursorNumber(state.cursor, ref.pdf.pages) : 1;
  }

  /** How the pair of columns `a`, `b` stand at the page on screen. */
  function pairStatus(pair: ComparePair): "matched" | "different" | null {
    return pairPageStatus(pair, pageFor(pair.a), pageFor(pair.b));
  }

  // ---- editing in place ----

  async function afterChange(): Promise<void> {
    try {
      await api.refreshDuplicateBooks(state.bookIds);
    } catch {
      // The review list notices the change by itself; this only keeps a resolve from being refused.
    }
    hooks.libraryChanged?.();
    await load();
  }

  async function saveField(id: number, field: "title" | "authors"): Promise<boolean> {
    const d = state.drafts[id];
    if (!d) return false;
    d.errors[field] = undefined;
    if (!isDirty(d, field)) return true;
    const changes: BookFieldChanges = field === "title" ? { title: d.title.trim() } : { authors: splitAuthors(d.authors) };
    if (field === "title" && !d.title.trim()) {
      d.errors.title = "A title cannot be empty.";
      return false;
    }
    if (field === "authors" && splitAuthors(d.authors).length === 0) {
      d.errors.authors = "Enter at least one author (separate several with &).";
      return false;
    }
    d.saving[field] = true;
    try {
      await api.setFields(id, changes);
      d.saved[field] = true;
      d.base[field] = d[field].trim();
      await afterChange();
      return true;
    } catch (e) {
      d.errors[field] = message(e);
      return false;
    } finally {
      d.saving[field] = false;
    }
  }

  const saveTitle = (id: number) => saveField(id, "title");
  const saveAuthors = (id: number) => saveField(id, "authors");

  /** Saves the title and the authors (the ones that changed), each with its own error. */
  async function saveMetadata(id: number): Promise<boolean> {
    const t = await saveTitle(id);
    const a = await saveAuthors(id);
    return t && a;
  }

  /** Copies this column's title and authors into every other column's boxes. They still need Save. */
  function useTitleForAll(id: number): void {
    const from = state.drafts[id];
    if (!from) return;
    for (const [key, d] of Object.entries(state.drafts)) {
      if (Number(key) === id) continue;
      d.title = from.title;
      d.authors = from.authors;
      d.errors.title = undefined;
      d.errors.authors = undefined;
      d.saved.title = false;
      d.saved.authors = false;
    }
  }

  /** Renames the file(s) on disk to this stem (a literal name through `/rename-files`). */
  async function saveStem(id: number): Promise<boolean> {
    const d = state.drafts[id];
    if (!d) return false;
    d.errors.stem = undefined;
    const problem = stemProblem(d.stem);
    if (problem) {
      d.errors.stem = problem;
      return false;
    }
    if (!isDirty(d, "stem")) return true;
    d.saving.stem = true;
    try {
      const out = await api.renameFiles([id], d.stem.trim(), false);
      const r = out.results[0];
      if (!r || r.error) {
        d.errors.stem = r?.error ?? "The file could not be renamed.";
        return false;
      }
      // A taken name comes back numbered: show the name it really has.
      if (r.proposed) d.stem = r.proposed;
      d.base.stem = d.stem;
      d.saved.stem = true;
      await afterChange();
      return true;
    } catch (e) {
      d.errors.stem = message(e);
      return false;
    } finally {
      d.saving.stem = false;
    }
  }

  /** Moves the book to the folder typed: a dry run, then the move itself. */
  async function moveBook(id: number): Promise<boolean> {
    const d = state.drafts[id];
    if (!d) return false;
    d.errors.folder = undefined;
    const folder = d.folder.trim();
    if (!folder) {
      d.errors.folder = "Enter a folder, relative to the library.";
      return false;
    }
    d.saving.folder = true;
    d.moveNote = "";
    try {
      const plan = await api.planMoveBooks([id], folder);
      if (plan.books === 0) {
        d.errors.folder = plan.skipped[0]?.reason ?? "The book is already in that folder.";
        return false;
      }
      d.moveNote = plan.conflicts > 0 ? "Moving (a file with that name is there already, so it is numbered)…" : "Moving…";
      const from = d.base.folder;
      const report = await api.runMoveBooks([id], plan.folder, plan.plan_fingerprint, { onProgress: (job) => (d.moveNote = describeJobProgress(job.progress)) });
      if (report.books_moved === 0) {
        d.errors.folder = report.failed[0]?.reason ?? "The book could not be moved.";
        d.moveNote = "";
        return false;
      }
      d.move = { runId: report.run_id, from, to: plan.folder };
      d.base.folder = plan.folder;
      d.moveNote = `Moved to ${plan.folder}.`;
      d.saved.folder = true;
      await afterChange();
      return true;
    } catch (e) {
      d.errors.folder = message(e);
      d.moveNote = "";
      return false;
    } finally {
      d.saving.folder = false;
    }
  }

  async function undoMove(id: number): Promise<boolean> {
    const d = state.drafts[id];
    if (!d?.move) return false;
    d.errors.folder = undefined;
    d.saving.folder = true;
    try {
      const done = await api.undoOrganize(d.move.runId);
      if (done.books_restored === 0) {
        d.errors.folder = done.skipped[0]?.reason ?? "The move could not be undone (the book changed since).";
        return false;
      }
      d.folder = d.move.from;
      d.base.folder = d.move.from;
      d.move = null;
      d.moveNote = "Move undone.";
      await afterChange();
      return true;
    } catch (e) {
      d.errors.folder = message(e);
      return false;
    } finally {
      d.saving.folder = false;
    }
  }

  // ---- the set: add, remove, group ----

  /** Looks up books to add. Books already in the set are left out. */
  async function searchBooks(query: string): Promise<void> {
    state.search.query = query;
    state.search.error = null;
    if (!query.trim()) {
      state.search.results = [];
      return;
    }
    state.search.busy = true;
    try {
      const found = await api.search({ query: query.trim(), num: 10, offset: 0, sort: "title", sortOrder: "asc", vl: "" });
      const books = await api.fetchBooks(found.book_ids.filter((id) => !state.bookIds.includes(id)));
      state.search.results = books.map((b) => ({ id: b.id, title: b.title, authors: b.authors ?? [] }));
    } catch (e) {
      state.search.error = message(e);
    } finally {
      state.search.busy = false;
    }
  }

  function adoptManual(res: ManualGroupResult): void {
    if (res.manual_group) state.manualGroupId = res.manual_group.id;
    else state.manualGroupId = null;
  }

  /** Adds a book to the set (and to the user's group, when the view is for one). */
  async function addBook(id: number): Promise<boolean> {
    state.error = null;
    if (state.bookIds.includes(id)) return true;
    if (state.bookIds.length >= MAX_BOOKS) {
      state.error = `Only ${MAX_BOOKS} books can be compared at once.`;
      return false;
    }
    try {
      let ids = [...state.bookIds, id];
      if (state.manualGroupId) {
        const res = await api.addToManualGroup(state.manualGroupId, [id]);
        adoptManual(res);
        // A book that was in another group of the user's brings that group along.
        const members = res.manual_group?.book_ids ?? [];
        ids = [...new Set([...ids, ...members])];
        if (ids.length > MAX_BOOKS) {
          state.error = `That group has ${ids.length} books; only ${MAX_BOOKS} can be compared at once.`;
          ids = ids.slice(0, MAX_BOOKS);
        }
        state.groupId = res.group?.id ?? null;
        state.groupBookIds = res.group?.books.map((b) => b.book_id) ?? [];
      } else {
        state.groupId = null;
      }
      state.bookIds = ids;
      state.search = { query: "", busy: false, results: [], error: null };
      await load();
      hooks.groupsChanged?.();
      return true;
    } catch (e) {
      state.error = message(e);
      return false;
    }
  }

  /** Takes a book out of the comparison (and out of the user's group, when the view is for one). */
  async function removeBook(id: number): Promise<boolean> {
    state.error = null;
    if (state.bookIds.length <= MIN_BOOKS) {
      state.error = `A comparison needs at least ${MIN_BOOKS} books.`;
      return false;
    }
    try {
      if (state.manualGroupId) {
        const res = await api.removeFromManualGroup(state.manualGroupId, [id]);
        adoptManual(res);
        state.groupId = res.group?.id ?? null;
        state.groupBookIds = res.group?.books.map((b) => b.book_id) ?? [];
        hooks.groupsChanged?.();
      } else {
        state.groupId = null;
        state.groupBookIds = [];
      }
      state.bookIds = state.bookIds.filter((b) => b !== id);
      if (state.keeperId === id) {
        state.keeperId = null;
        state.keeperChosen = false;
      }
      await load();
      return true;
    } catch (e) {
      state.error = message(e);
      return false;
    }
  }

  /** "Mark these as duplicates": the set becomes a group of the user's own. */
  async function saveAsGroup(): Promise<boolean> {
    state.error = null;
    state.groupBusy = true;
    try {
      const res = await api.createManualGroup(state.bookIds);
      adoptManual(res);
      state.groupId = res.group?.id ?? null;
      state.groupBookIds = res.group?.books.map((b) => b.book_id) ?? [];
      const members = res.manual_group?.book_ids ?? [];
      const extra = members.filter((m) => !state.bookIds.includes(m));
      if (extra.length > 0) {
        // The books were in a group already: it is shown whole.
        state.bookIds = [...state.bookIds, ...extra].slice(0, MAX_BOOKS);
        state.note = `Some of these books were already marked as duplicates together, so this now shows all ${members.length} of that set.`;
        await load();
      } else {
        state.note = "Marked as duplicates. They are in the review list as “Marked by you”.";
      }
      hooks.groupsChanged?.();
      return true;
    } catch (e) {
      state.error = message(e);
      return false;
    } finally {
      state.groupBusy = false;
    }
  }

  /** "Not duplicates" for the whole set. */
  async function notDuplicates(): Promise<boolean> {
    state.error = null;
    try {
      await api.markNotDuplicates({ bookIds: state.bookIds });
      state.note = "Marked as not duplicates: they will not be suggested as a group again.";
      state.decision = { ...state.decision, phase: "done", message: state.note };
      state.manualGroupId = null;
      state.groupId = null;
      hooks.groupsChanged?.();
      await load();
      return true;
    } catch (e) {
      state.error = message(e);
      return false;
    }
  }

  // ---- deciding ----

  function chooseKeeper(id: number): void {
    state.keeperId = id;
    state.keeperChosen = id !== state.result?.suggested_keeper;
  }

  /**
   * The review list's group that holds exactly the books on screen, making
   * the set a group of the user's own if it is not one. `null` (with
   * `state.error` saying why) when it cannot be done, in which case nothing
   * has been trashed and a group made only for this is taken away again.
   */
  async function ensureGroup(): Promise<{ id: string; createdManual: string | null } | null> {
    if (state.groupId && sameSet(state.groupBookIds, state.bookIds)) return { id: state.groupId, createdManual: null };
    await api.refreshDuplicateBooks(state.bookIds).catch(() => undefined);
    const existing = new Set((await api.fetchManualGroups()).groups.map((g) => g.id));
    let res = await api.createManualGroup(state.bookIds);
    const manualId = res.manual_group?.id ?? null;
    const createdManual = manualId && !existing.has(manualId) ? manualId : null;
    adoptManual(res);
    if (!res.group) {
      // No scan to resolve from yet: make one (it includes the group just made).
      state.decision.progress = "Scanning the library…";
      await api.runDuplicateScan("fuzzy", { onProgress: (job) => (state.decision.progress = describeJobProgress(job.progress)) });
      const listed = (await api.fetchManualGroups()).groups.find((g) => g.id === manualId);
      const gid = listed?.snapshot_group_id ?? null;
      res = { ...res, group: gid ? ({ id: gid, books: (listed?.book_ids ?? []).map((b) => ({ book_id: b })) } as unknown as DuplicateGroup) : null };
    }
    const group = res.group;
    const members = group ? group.books.map((b) => b.book_id) : [];
    if (!group?.id || !sameSet(members, state.bookIds)) {
      if (createdManual) await api.deleteManualGroup(createdManual).catch(() => undefined);
      state.error = group?.id
        ? `These books are part of a larger group of ${members.length}. Nothing was trashed: compare that whole group, or remove the books you do not want in it.`
        : "The duplicate scan could not make a group of exactly these books. Nothing was trashed.";
      return null;
    }
    state.groupId = group.id;
    state.groupBookIds = members;
    return { id: group.id, createdManual };
  }

  /**
   * "Keep selected, trash the others": the resolve route for exactly this set.
   * `confirm` is shown the plan's summary and decides whether to go on.
   * `mergeOthers` merges the others' metadata and formats into the kept copy
   * before they go (the resolve action `merge_as_formats`).
   */
  async function keepAndTrash(options: { mergeOthers?: boolean; confirm: (summary: string, plan: ResolvePlan) => boolean | Promise<boolean> }): Promise<boolean> {
    const keeper = state.keeperId;
    if (keeper === null || !state.bookIds.includes(keeper)) {
      state.error = "Choose the copy to keep first.";
      return false;
    }
    state.error = null;
    state.decision = { phase: "working", message: "Preparing…", runId: null, trashedIds: [], progress: "", jobId: null };
    let made: { id: string; createdManual: string | null } | null = null;
    try {
      made = await ensureGroup();
      if (!made) {
        state.decision.phase = "error";
        return false;
      }
      const action = options.mergeOthers ? "merge_as_formats" : "keep_best_trash_rest";
      const selection: ResolveSelection = { action, groupIds: [made.id], keepers: { [made.id]: keeper } };
      const plan = await api.planResolve(selection);
      const planned = plan.groups[0];
      const others = state.bookIds.filter((b) => b !== keeper).sort((a, b) => a - b);
      if (plan.groups.length !== 1 || !planned || planned.keeper.book_id !== keeper || !sameSet(planned.others.map((b) => b.book_id), others)) {
        throw new Error(plan.skipped[0]?.reason ?? "The server's plan is not for exactly these books, so nothing was trashed.");
      }
      const titles = state.result?.books.filter((b) => others.includes(b.book_id)).map((b) => `“${b.title}”`) ?? [];
      const summary = options.mergeOthers
        ? `Merge ${titles.join(", ")} into the kept copy, then move ${others.length === 1 ? "it" : "them"} to the library trash?`
        : `Keep this copy and move ${titles.join(", ")} to the library trash? You can undo this from here, or restore from Library → Trash….`;
      if (!(await options.confirm(summary, plan))) {
        if (made.createdManual) await api.deleteManualGroup(made.createdManual).catch(() => undefined);
        state.decision = { phase: "idle", message: "", runId: null, trashedIds: [], progress: "", jobId: null };
        return false;
      }
      state.decision.message = "Working…";
      const report = await api.runResolve(selection, plan.plan_fingerprint, {
        onStart: (id) => (state.decision.jobId = id),
        onProgress: (job) => (state.decision.progress = describeJobProgress(job.progress)),
      });
      if (report.books_trashed < others.length) {
        throw new Error(report.errors[0]?.[1] ?? "Not every other copy could be trashed.");
      }
      state.decision = { phase: "done", message: options.mergeOthers ? `Merged ${report.books_trashed} into the kept copy and trashed them.` : `Trashed ${report.books_trashed} ${report.books_trashed === 1 ? "copy" : "copies"}.`, runId: report.run_id, trashedIds: others, progress: "", jobId: null };
      state.bookIds = [keeper];
      state.groupId = null;
      state.manualGroupId = null;
      hooks.libraryChanged?.();
      await hooks.groupsChanged?.();
      return true;
    } catch (e) {
      state.error = message(e);
      state.decision = { phase: "error", message: "", runId: null, trashedIds: [], progress: "", jobId: null };
      if (made?.createdManual) await api.deleteManualGroup(made.createdManual).catch(() => undefined);
      return false;
    }
  }

  /** Undoes the trashing (the resolve run's restore) and shows the whole set again. */
  async function undoDecision(): Promise<boolean> {
    const runId = state.decision.runId;
    if (!runId) return false;
    state.error = null;
    try {
      const done = await api.restoreResolveRun(runId);
      if (done.restored.length === 0) {
        state.error = done.failed[0]?.[1] ?? "Nothing could be restored.";
        return false;
      }
      state.bookIds = [...new Set([...state.bookIds, ...done.restored.map((r) => r.book_id)])];
      state.decision = { phase: "idle", message: `Restored ${done.restored.length} ${done.restored.length === 1 ? "book" : "books"}.`, runId: null, trashedIds: [], progress: "", jobId: null };
      hooks.libraryChanged?.();
      await hooks.groupsChanged?.();
      await load();
      return true;
    } catch (e) {
      state.error = message(e);
      return false;
    }
  }

  /** After an Undo made outside the view (the toast shown when it closed): the library and the review list changed. */
  async function refreshAfterUndo(): Promise<void> {
    hooks.libraryChanged?.();
    await hooks.groupsChanged?.();
  }

  return {
    state,
    open,
    close,
    refreshAfterUndo,
    load,
    pagesOf,
    pageFor,
    pageNumber,
    pairStatus,
    first,
    middle,
    last,
    step,
    goTo,
    setAlignFromEnd,
    saveTitle,
    saveAuthors,
    saveMetadata,
    useTitleForAll,
    saveStem,
    moveBook,
    undoMove,
    searchBooks,
    addBook,
    removeBook,
    saveAsGroup,
    notDuplicates,
    chooseKeeper,
    keepAndTrash,
    undoDecision,
  };
}

export type CompareController = ReturnType<typeof createCompareController>;
