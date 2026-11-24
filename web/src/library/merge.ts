// Merge books: choosing a destination, and saying what a merge will do.
//
// Pure functions only, so the rules can be tested without a component.
// The server decides what is actually done (crates/lexmechanic_db/src/merge.rs);
// this module (a) pre-selects the record a person is most likely to want
// to keep and (b) turns a dry-run response into a confirmation they can
// read before anything is changed.

import type { DuplicateBook, MergeMode, MergeResponse } from "./api";

/**
 * The record to merge the others into. Mirrors
 * `lexmechanic_db::merge::best_destination`: most formats, then most filled-in
 * metadata, then has a cover, then the oldest (lowest id). A person can
 * always pick another; this is only the default.
 */
export function defaultMergeDestination(books: readonly DuplicateBook[]): number | null {
  let best: DuplicateBook | null = null;
  for (const b of books) {
    if (best === null || compareRecords(b, best) > 0) best = b;
  }
  return best ? best.book_id : null;
}

function compareRecords(a: DuplicateBook, b: DuplicateBook): number {
  const formats = (a.formats ?? 0) - (b.formats ?? 0);
  if (formats !== 0) return formats;
  const metadata = (a.metadata ?? 0) - (b.metadata ?? 0);
  if (metadata !== 0) return metadata;
  const cover = Number(a.has_cover ?? false) - Number(b.has_cover ?? false);
  if (cover !== 0) return cover;
  // Older record wins a tie: a lower id sorts higher.
  return b.book_id - a.book_id;
}

/** The tooltip that explains the default destination. */
export const DEFAULT_DESTINATION_HINT = "Pre-selected: the record with the most formats, then the most metadata filled in, then one that has a cover, then the oldest. Choose another if you prefer.";

/** One line saying why this book is the suggested destination, for its row. */
export function describeRecord(b: DuplicateBook): string {
  const parts: string[] = [];
  if (b.formats !== undefined) parts.push(`${b.formats} format${b.formats === 1 ? "" : "s"}`);
  if (b.metadata !== undefined) parts.push(`${b.metadata} metadata field${b.metadata === 1 ? "" : "s"}`);
  if (b.has_cover !== undefined) parts.push(b.has_cover ? "has cover" : "no cover");
  return parts.join(", ");
}

export const MERGE_MODE_LABELS: Record<MergeMode, string> = {
  merge_and_delete: "Merge into the destination and remove the others",
  merge_keep_sources: "Merge into the destination, keep the others",
  formats_only: "Move only the formats, remove the others",
};

export const MERGE_MODE_NOTES: Record<MergeMode, string> = {
  merge_and_delete: "Formats and missing metadata move to the destination. The other books are removed from the library, and their leftover files go to the library trash (Library → Trash…). A merge cannot be undone as a whole: to keep one copy and be able to undo, use “Keep this, trash the others” instead.",
  merge_keep_sources: "Formats and missing metadata are copied to the destination. The other books are not changed or deleted.",
  formats_only: "Only formats move. The destination's metadata is not touched, and the other books are removed with their leftover files going to the library trash.",
};

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * The confirmation shown after a dry run: what moves, what is kept
 * because the destination already has it, and what goes to the
 * library trash. One sentence per fact, nothing the user has to decode.
 */
export function summarizeMerge(r: MergeResponse): string[] {
  const lines: string[] = [];
  const moved = r.results.flatMap((s) => s.formats_moved.map((f) => `${f} from "${s.title}"`));
  lines.push(moved.length ? `Formats added to the destination: ${moved.join(", ")}.` : "No formats need to move.");

  const identical = r.results.reduce((n, s) => n + s.formats_identical.length, 0);
  if (identical) lines.push(`${plural(identical, "format")} already in the destination with the same content (nothing to add).`);

  const discarded = r.results.flatMap((s) => s.formats_discarded.map((f) => `${f} of "${s.title}"`));
  if (discarded.length) lines.push(`The destination's own copy is kept; these different copies are discarded: ${discarded.join(", ")}.`);

  const kept = r.results.reduce((n, s) => n + s.formats_kept_as_data_file.length, 0);
  if (kept) lines.push(`${plural(kept, "different copy", "different copies")} saved as data files in the destination.`);

  const left = r.results.reduce((n, s) => n + s.formats_left_on_source.length, 0);
  if (left) lines.push(`${plural(left, "different copy", "different copies")} stay on the books that are kept.`);

  if (r.mode !== "formats_only") {
    lines.push(r.metadata_changed.length ? `Metadata filled in or combined: ${r.metadata_changed.join(", ")}.` : "No metadata changes.");
  }
  if (r.cover_from !== null) lines.push(`The destination takes the cover of "${r.results.find((s) => s.book_id === r.cover_from)?.title ?? r.cover_from}".`);

  if (r.mode === "merge_keep_sources") {
    lines.push("The other books are kept.");
  } else {
    const deleted = r.results.filter((s) => s.deleted).length;
    lines.push(`${plural(deleted, "book")} will be removed from the library.`);
    lines.push(r.trashed.length ? `${plural(r.trashed.length, "file")} will move to the library trash: ${r.trashed.join(", ")}.` : "No files go to the library trash (Library → Trash…).");
  }
  for (const s of r.results) if (s.error) lines.push(`Cannot merge "${s.title}": ${s.error}`);
  for (const w of r.warnings) lines.push(`Note: ${w}`);
  return lines;
}

/** After a real merge: what to tell the user. */
export function describeMergeResult(r: MergeResponse): string {
  const merged = r.results.filter((s) => !s.error).length;
  const parts = [`Merged ${plural(merged, "book")}.`];
  if (r.trashed.length) parts.push(`${plural(r.trashed.length, "file")} moved to the library trash.`);
  if (r.failed.length) parts.push(`Left in place because it could not be trashed: ${r.failed.map((f) => f.path).join(", ")}.`);
  for (const e of r.errors) parts.push(`Book ${e.book_id} was not merged: ${e.error}`);
  return parts.join(" ");
}
