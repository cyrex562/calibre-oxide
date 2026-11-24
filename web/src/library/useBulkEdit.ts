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
import { inverseRelativeOp, relativeTagOp, runRelativeEdit, type RelativeOp } from "./bulkRelative";
import { changesFor, describeBulkResult, inverseChanges, isEmptySpec, uniformChanges, validateSpec, type BulkEditSpec } from "./bulkEdit";
import { showToast } from "./deleteFlow";

export interface BulkEditDeps {
  selectedIds: Ref<Set<number>>;
  selectMode: Ref<boolean>;
  fieldMetadata: Ref<Record<string, FieldMetaEntry>>;
  /** The loaded books: `ensureLoaded` fetches the ones a selection reached without scrolling to them (and returns them: the cache is capped). */
  win: { ensureLoaded(ids: number[]): Promise<BookSummary[]>; getById(id: number): BookSummary | undefined };
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

export function useBulkEdit({ selectedIds, fieldMetadata, win, onChanged }: BulkEditDeps) {
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
  async function runRelative(ids: number[], op: RelativeOp, edit: BulkEditSpec): Promise<boolean> {
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
      // The selection stays, as after any other bulk edit: a toast is the feedback.
      open.value = false;
      spec.value = emptySpec();
      applyRating.value = false;
      applySeriesIndex.value = false;
    }
    await onChanged();
    if (pool.updated > 0) {
      const message = describeBulkResult(edit, pool.updated);
      const inverse = inverseRelativeOp(op);
      const changedIds = pool.updatedIds;
      if (!inverse) showToast(message, undefined, 15000);
      else
        showToast(
          message,
          {
            label: "Undo",
            run: async () => {
              const back = await runRelativeEdit(changedIds, inverse, { send: setFieldsRelative });
              await onChanged();
              if (!back) throw new Error("The server could not undo this edit");
              if (back.errors.length > 0) throw new Error(`Put back ${back.updated} of ${changedIds.length}: ${back.errors[0]}`);
              return `Put back the tags on ${back.updated.toLocaleString("en-US")} ${back.updated === 1 ? "book" : "books"}`;
            },
          },
          20000,
        );
    }
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
    if (relative && (await runRelative(ids, relative, edit))) return;

    busy.value = true;
    errors.value = [];
    summary.value = null;

    let changed = 0;
    let skipped = 0;
    cancelled = false;
    // What each book needs is worked out here, per book, before anything is sent.
    const items: BulkItem[] = [];
    // What each written book held before, so the edit can be undone: the old value of exactly the fields written.
    const before: BulkItem[] = [];
    let lostFields = new Set<string>();
    // An edit that gives every book the same values (set the publisher, the series, a rating...) does
    // not depend on what a book has now: no row is read first, and the server says what it replaced.
    const uniform = uniformChanges(edit);
    if (uniform) {
      for (const id of ids) items.push({ id, changes: uniform });
    } else {
      // A selection can reach rows that were never scrolled to (select
      // all, a long shift-range): fetch the ones that are not loaded.
      let loaded = new Map<number, BookSummary>();
      try {
        loaded = new Map((await win.ensureLoaded(ids)).map((b) => [b.id, b]));
      } catch (e) {
        errors.value.push(`Could not read the selected books: ${e instanceof Error ? e.message : String(e)}`);
      }
      for (const id of ids) {
        const current = loaded.get(id) ?? win.getById(id);
        if (!current) {
          errors.value.push(`Book ${id}: could not be loaded`);
          continue;
        }
        try {
          const changes = changesFor(current, edit);
          if (changes) {
            items.push({ id, changes });
            const inverse = inverseChanges(current, changes);
            if (Object.keys(inverse.changes).length > 0) before.push({ id, changes: inverse.changes });
            lostFields = new Set([...lostFields, ...inverse.lost]);
          } else skipped += 1;
        } catch (e) {
          errors.value.push(`Book ${id}: ${e instanceof Error ? e.message : String(e)}`);
        }
      }
    }
    progress.value = { done: 0, total: items.length };
    const pool = await runBatchedEdit(items, {
      sendBatch: uniform ? (groups) => setFieldsBatch(groups, { previous: true }) : (groups) => setFieldsBatch(groups),
      sendOne: setFields,
      cancelled: () => cancelled,
      onProgress: (done, total) => (progress.value = { done, total }),
    });
    if (uniform) {
      // Undo from what the server said each book held (a server that does not say leaves no Undo).
      for (const [id, row] of pool.previous) {
        const inverse = inverseChanges({ id, ...row } as BookSummary, uniform);
        if (Object.keys(inverse.changes).length > 0) before.push({ id, changes: inverse.changes });
        lostFields = new Set([...lostFields, ...inverse.lost]);
      }
    }
    changed = pool.updated;
    errors.value.push(...pool.errors);
    busy.value = false;
    progress.value = null;
    const stopped = pool.notStarted > 0 ? `, stopped with ${pool.notStarted} not started` : "";
    summary.value = `Updated ${changed} book(s)${skipped ? `, ${skipped} already matched` : ""}${errors.value.length ? `, ${errors.value.length} failed` : ""}${stopped}.`;

    if (errors.value.length === 0 && pool.notStarted === 0) {
      // The selection stays: the same books are often edited again (a tag, then a series), and a toast is the feedback.
      open.value = false;
      spec.value = emptySpec();
      applyRating.value = false;
      applySeriesIndex.value = false;
    }
    await onChanged();
    if (changed > 0) announce(describeBulkResult(edit, changed), before, pool.errors, lostFields);
  }

  /**
   * The toast after an edit: what was done, with an Undo that writes the old
   * values back through the same batch route. Books that failed are left out.
   */
  function announce(message: string, before: BulkItem[], failures: string[], lost: Set<string>) {
    const failed = new Set(failures.map((f) => /^Book (\d+):/.exec(f)?.[1]).filter((id): id is string => !!id).map(Number));
    const restore = before.filter((b) => !failed.has(b.id));
    if (restore.length === 0) {
      showToast(message, undefined, 15000);
      return;
    }
    const note = lost.size > 0 ? ` (${[...lost].join(", ")} cannot be unset, so Undo leaves ${lost.size === 1 ? "it" : "them"} as edited)` : "";
    showToast(
      message,
      {
        label: "Undo",
        run: async () => {
          const back = await runBatchedEdit(restore, { sendBatch: setFieldsBatch, sendOne: setFields });
          await onChanged();
          if (back.errors.length > 0) throw new Error(`Put back ${back.updated} of ${restore.length}: ${back.errors[0]}`);
          return `Put back the old values on ${back.updated.toLocaleString("en-US")} ${back.updated === 1 ? "book" : "books"}${note}`;
        },
      },
      20000,
    );
  }

  // Reactive, so a template (and a component given this as a prop) reads and
  // writes the refs without `.value`.
  return reactive({ open, busy, errors, progress, spec, applyRating, rating, applySeriesIndex, seriesIndex, summary, customColumns, run, cancel });
}

export type BulkEdit = ReturnType<typeof useBulkEdit>;
