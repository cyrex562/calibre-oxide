// "Add tags" / "remove tags" on many books without reading them first.
//
// The usual bulk edit computes each book's new tag list in the browser, which
// needs every book's current tags (hundreds of /ajax/books requests for a big
// selection) and then sends a different list per book. The server can do this
// itself: `POST /cdb/set-fields-batch` takes `{book_ids, add: {tags: [...]}}` /
// `{book_ids, remove: {tags: [...]}}`, reads each book's tags and writes the
// lot in a few transactions. This module decides when an edit is exactly that
// and sends it in slices; anything else, or a server without the operation,
// goes the old way.
//
// Pure of Vue and `fetch`: the sender is passed in.

import { parseList, type BulkEditSpec } from "./bulkEdit";

/** Books per request. The server commits about 1,000 per transaction. */
export const RELATIVE_CHUNK = 2000;

export interface RelativeOp {
  field: "tags";
  mode: "add" | "remove";
  values: string[];
}

export interface RelativeResponse {
  updated: number[];
  failed: { book_id: number; error: string }[];
  /** Books that already had the result. */
  unchanged?: number[];
  /** Per changed book, the exact spellings the edit added to and removed from each list. */
  changes?: Record<string, Record<string, { added: string[]; removed: string[] }>>;
}

/** Thrown by a sender when the server does not know add/remove (an older server). */
export class RelativeUnsupportedError extends Error {
  constructor() {
    super("this server has no add/remove tags operation");
    this.name = "RelativeUnsupportedError";
  }
}

/**
 * The relative operation `spec` amounts to, or `null` when it is anything else
 * (another field changed, a search-and-replace, replacing the tags outright).
 */
export function relativeTagOp(spec: BulkEditSpec): RelativeOp | null {
  const tags = spec.tags;
  if (!tags || (tags.mode !== "add" && tags.mode !== "remove")) return null;
  const values = parseList(tags.value);
  if (values.length === 0) return null;

  const touches = (e: { value: string } | undefined) => !!e && parseList(e.value).length > 0;
  if (touches(spec.authors) || touches(spec.languages)) return null;
  if (spec.publisher || spec.series || spec.pubdate || spec.comments) return null;
  if (spec.seriesIndex !== null && spec.seriesIndex !== undefined) return null;
  if (spec.rating !== null && spec.rating !== undefined) return null;
  if (Object.values(spec.custom ?? {}).some((v) => v !== "")) return null;
  if (spec.searchReplace?.find) return null;
  return { field: "tags", mode: tags.mode, values };
}

export interface RelativeRunDeps {
  send: (bookIds: number[], op: RelativeOp) => Promise<RelativeResponse>;
  cancelled?: () => boolean;
  onProgress?: (done: number, total: number) => void;
  chunkSize?: number;
}

export interface RelativeRunResult {
  updated: number;
  /** The books the server changed, for an undo of exactly those. */
  updatedIds: number[];
  /** What each changed book gained and lost in the field (absent from an older server). */
  deltas: Map<number, { added: string[]; removed: string[] }>;
  unchanged: number;
  errors: string[];
  notStarted: number;
}

/**
 * Sends `op` for `ids`, slice by slice. Returns `null` when the server does not
 * support it and nothing was sent (the caller then edits the old way); once a
 * slice has been written the run goes on to the end, since falling back half
 * way would redo books that are already done.
 */
export async function runRelativeEdit(ids: readonly number[], op: RelativeOp, deps: RelativeRunDeps): Promise<RelativeRunResult | null> {
  const size = Math.max(1, deps.chunkSize ?? RELATIVE_CHUNK);
  const result: RelativeRunResult = { updated: 0, updatedIds: [], deltas: new Map(), unchanged: 0, errors: [], notStarted: 0 };
  let at = 0;
  let sentAny = false;
  while (at < ids.length) {
    if (deps.cancelled?.()) {
      result.notStarted = ids.length - at;
      break;
    }
    const slice = ids.slice(at, at + size);
    try {
      const response = await deps.send(slice, op);
      sentAny = true;
      result.updated += response.updated.length;
      result.updatedIds.push(...response.updated);
      for (const [id, fields] of Object.entries(response.changes ?? {})) {
        const delta = fields[op.field];
        if (delta) result.deltas.set(Number(id), delta);
      }
      result.unchanged += response.unchanged?.length ?? 0;
      for (const f of response.failed) result.errors.push(`Book ${f.book_id}: ${f.error}`);
    } catch (e) {
      if (e instanceof RelativeUnsupportedError && !sentAny) return null;
      const message = e instanceof Error ? e.message : String(e);
      for (const id of slice) result.errors.push(`Book ${id}: ${message}`);
      sentAny = true;
    }
    at += slice.length;
    deps.onProgress?.(at, ids.length);
  }
  return result;
}

/**
 * The op that undoes `op` on the books it changed, or `null` when that cannot
 * be exact: adding two tags to a book that had one of them changes the book,
 * and removing both again would take away the one it already had. Only the
 * single-tag case is exact without knowing what each book gained or lost; see
 * {@link inverseRelativeOps} for the exact, per-book undo.
 */
export function inverseRelativeOp(op: RelativeOp): RelativeOp | null {
  if (op.values.length !== 1) return null;
  return { ...op, mode: op.mode === "add" ? "remove" : "add" };
}

export interface RelativeUndoStep {
  ids: number[];
  op: RelativeOp;
}

/**
 * The steps that undo a finished relative edit exactly: per book, an add is
 * undone by removing only the spellings that book gained, a remove by adding
 * back only the spellings it lost. Books with the same delta share a step.
 * Falls back to the single-tag inverse for a server that did not say what each
 * book gained and lost; `null` when no exact undo exists.
 */
export function inverseRelativeOps(op: RelativeOp, result: Pick<RelativeRunResult, "updatedIds" | "deltas">): RelativeUndoStep[] | null {
  if (result.deltas.size === 0) {
    const inverse = inverseRelativeOp(op);
    return inverse ? [{ ids: result.updatedIds, op: inverse }] : null;
  }
  const mode = op.mode === "add" ? "remove" : "add";
  const groups = new Map<string, RelativeUndoStep>();
  for (const id of result.updatedIds) {
    const delta = result.deltas.get(id);
    if (!delta) return null; // a changed book we know nothing about: no exact undo
    const values = op.mode === "add" ? delta.added : delta.removed;
    if (values.length === 0) continue;
    const key = JSON.stringify(values);
    const group = groups.get(key);
    if (group) group.ids.push(id);
    else groups.set(key, { ids: [id], op: { ...op, mode, values } });
  }
  return [...groups.values()];
}
