// Bulk (multi-book) metadata editing: what the panel edits and what "Apply"
// does. The changes are sent in chunks of ~200 books to the server's batch route
// (./bulkBatch.ts), with per-book error aggregation so one bad book doesn't
// silently swallow the rest. A server without the route gets the old
// one-request-per-book fan-out.
//
// Bulk edit (#1.6). The previous version reached add-tags, remove-tags,
// set-series and a rating; `Cache::set_field` accepts authors, publisher,
// languages, pubdate, comments, series_index and any custom column too -- which
// is most of what tidying an imported pile of PDFs actually needs.
//
// The change computation lives in ./bulkEdit.ts: deciding what to write is where
// the bugs are, and they are invisible here until the wrong thing has already
// been written to a hundred books.

import { computed, reactive, ref, type Ref } from "vue";

import type { BookSummary, FieldMetaEntry } from "./types";
import { setFields, setFieldsBatch, setFieldsRelative } from "./api";
import { runBulkEdit as runBatchedEdit, type BulkItem } from "./bulkBatch";
import { relativeTagOp, runRelativeEdit, type RelativeOp } from "./bulkRelative";
import { changesFor, isEmptySpec, validateSpec, type BulkEditSpec } from "./bulkEdit";

export interface BulkEditDeps {
  selectedIds: Ref<Set<number>>;
  selectMode: Ref<boolean>;
  fieldMetadata: Ref<Record<string, FieldMetaEntry>>;
  /** The loaded books: `ensureLoaded` fetches the ones a selection reached without scrolling to them. */
  win: { ensureLoaded(ids: number[]): Promise<unknown>; getById(id: number): BookSummary | undefined };
  /** The edit changed books: bring every view up to date. */
  onChanged: () => Promise<void>;
}

export function emptySpec(): BulkEditSpec {
  return {
    tags: { mode: "add", value: "" },
    authors: { mode: "add", value: "" },
    languages: { mode: "add", value: "" },
    publisher: "",
    series: "",
    seriesIndex: null,
    rating: null,
    pubdate: "",
    comments: "",
    custom: {},
    searchReplace: { field: "title", find: "", replace: "", useRegex: false },
  };
}

export function useBulkEdit({ selectedIds, selectMode, fieldMetadata, win, onChanged }: BulkEditDeps) {
  const open = ref(false);
  const busy = ref(false);
  const errors = ref<string[]>([]);
  /** "12 of 300" while a bulk edit runs. */
  const progress = ref<{ done: number; total: number } | null>(null);
  let cancelled = false;
  function cancel() {
    cancelled = true;
  }

  const spec = ref<BulkEditSpec>(emptySpec());
  const applyRating = ref(false);
  const rating = ref(0);
  const applySeriesIndex = ref(false);
  const seriesIndex = ref(1);
  const summary = ref<string | null>(null);

  /** Custom columns offered in the bulk panel, by their row key. */
  const customColumns = computed(() => Object.values(fieldMetadata.value).filter((f) => f.is_custom && f.label));

  /**
   * "Add tags" / "remove tags" alone: the server reads each book's tags, so none
   * are fetched here (./bulkRelative.ts). False when the server cannot do it,
   * and nothing was changed: the caller then edits the old way.
   */
  async function runRelative(ids: number[], op: RelativeOp): Promise<boolean> {
    busy.value = true;
    errors.value = [];
    summary.value = null;
    cancelled = false;
    progress.value = { done: 0, total: ids.length };
    const pool = await runRelativeEdit(ids, op, {
      send: setFieldsRelative,
      cancelled: () => cancelled,
      onProgress: (done, total) => (progress.value = { done, total }),
    });
    busy.value = false;
    progress.value = null;
    if (!pool) return false;
    errors.value.push(...pool.errors);
    const stopped = pool.notStarted > 0 ? `, stopped with ${pool.notStarted} not started` : "";
    summary.value = `Updated ${pool.updated} book(s)${pool.unchanged ? `, ${pool.unchanged} already matched` : ""}${errors.value.length ? `, ${errors.value.length} failed` : ""}${stopped}.`;
    if (errors.value.length === 0 && pool.notStarted === 0) {
      open.value = false;
      selectMode.value = false;
      selectedIds.value = new Set();
      spec.value = emptySpec();
      applyRating.value = false;
      applySeriesIndex.value = false;
    }
    await onChanged();
    return true;
  }

  async function run() {
    const ids = [...selectedIds.value];
    if (ids.length === 0) return;

    const edit: BulkEditSpec = {
      ...spec.value,
      rating: applyRating.value ? rating.value : null,
      seriesIndex: applySeriesIndex.value ? seriesIndex.value : null,
    };

    // Validate once, before touching a single book -- a bad regex
    // found on book 57 of 200 leaves the edit half applied.
    const problem = validateSpec(edit);
    if (problem) {
      errors.value = [problem];
      return;
    }
    if (isEmptySpec(edit)) {
      errors.value = ["Nothing to apply -- fill in at least one field."];
      return;
    }
    const relative = relativeTagOp(edit);
    if (relative && (await runRelative(ids, relative))) return;

    busy.value = true;
    errors.value = [];
    summary.value = null;

    let changed = 0;
    let skipped = 0;
    // A selection can reach rows that were never scrolled to (select
    // all, a long shift-range): fetch the ones that are not loaded.
    try {
      await win.ensureLoaded(ids);
    } catch (e) {
      errors.value.push(`Could not read the selected books: ${e instanceof Error ? e.message : String(e)}`);
    }
    cancelled = false;
    // What each book needs is worked out here, per book, before anything is sent.
    const items: BulkItem[] = [];
    for (const id of ids) {
      const current = win.getById(id);
      if (!current) {
        errors.value.push(`Book ${id}: could not be loaded`);
        continue;
      }
      try {
        const changes = changesFor(current, edit);
        if (changes) items.push({ id, changes });
        else skipped += 1;
      } catch (e) {
        errors.value.push(`Book ${id}: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    progress.value = { done: 0, total: items.length };
    const pool = await runBatchedEdit(items, {
      sendBatch: setFieldsBatch,
      sendOne: setFields,
      cancelled: () => cancelled,
      onProgress: (done, total) => (progress.value = { done, total }),
    });
    changed = pool.updated;
    errors.value.push(...pool.errors);
    busy.value = false;
    progress.value = null;
    const stopped = pool.notStarted > 0 ? `, stopped with ${pool.notStarted} not started` : "";
    summary.value = `Updated ${changed} book(s)${skipped ? `, ${skipped} already matched` : ""}${errors.value.length ? `, ${errors.value.length} failed` : ""}${stopped}.`;

    if (errors.value.length === 0 && pool.notStarted === 0) {
      open.value = false;
      selectMode.value = false;
      selectedIds.value = new Set();
      spec.value = emptySpec();
      applyRating.value = false;
      applySeriesIndex.value = false;
    }
    await onChanged();
  }

  // Reactive, so a template (and a component given this as a prop) reads and
  // writes the refs without `.value`.
  return reactive({ open, busy, errors, progress, spec, applyRating, rating, applySeriesIndex, seriesIndex, summary, customColumns, run, cancel });
}

export type BulkEdit = ReturnType<typeof useBulkEdit>;
