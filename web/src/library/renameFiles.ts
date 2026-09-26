// Renaming a book's files on disk (issue #885).
//
// The decisions here, rather than in the dialog, because they are the
// ones worth testing: which template the box starts with, whether the
// preview on screen still describes what Apply would do, and what the
// result of a batch actually was.
//
// A rename cannot be undone. That single fact is why "is this preview
// still valid" is a function with tests rather than a `v-if` -- an
// Apply button that stays enabled after the template changed underneath
// it would move files to names nobody ever saw.

import type { RenameFileResult } from "./types";

/**
 * Used when renaming several books at once, where one literal name
 * cannot apply to all of them. Matches the trailing component of the
 * default save-to-disk template, so the two features name files the
 * same way by default.
 */
export const DEFAULT_RENAME_TEMPLATE = "{title} - {authors}";

/**
 * What the name box starts with.
 *
 * For a single book it is that book's current filename, because the
 * overwhelmingly common case is fixing one name by hand and the most
 * useful starting point is what it is called now. For several, no
 * literal can serve, so it is a template.
 *
 * `stems` holds each selected book's current filename stem; an empty
 * string stands for a book whose formats disagree or that has none.
 */
export function initialRenameTemplate(stems: string[]): string {
  if (stems.length === 1 && stems[0]) return stems[0];
  return DEFAULT_RENAME_TEMPLATE;
}

/** The request a preview was computed for. */
export interface RenamePreviewKey {
  template: string;
  bookIds: number[];
}

/**
 * Whether a preview still describes what applying would do right now.
 *
 * False once the template is edited or the selection changes -- both
 * of which alter the names that would be written, so the table on
 * screen is no longer a promise about anything.
 */
export function previewIsCurrent(previewedFor: RenamePreviewKey | null, template: string, bookIds: number[]): boolean {
  if (!previewedFor) return false;
  if (previewedFor.template !== template) return false;
  if (previewedFor.bookIds.length !== bookIds.length) return false;
  // Order is not meaningful in a selection, but it does decide which
  // book wins a name collision -- so a reordered selection can produce
  // different names and must invalidate.
  return previewedFor.bookIds.every((id, i) => id === bookIds[i]);
}

export interface RenameSummary {
  /** Books whose files would move, or did. */
  changed: number;
  /** Books already called the right thing. */
  unchanged: number;
  /** Books the server could not name. */
  failed: number;
}

export function summarizeRename(results: RenameFileResult[]): RenameSummary {
  let changed = 0;
  let unchanged = 0;
  let failed = 0;
  for (const r of results) {
    // Checked before `changed`: a book that failed is reported with
    // `changed: false`, and counting it as "already correct" would be
    // the one reading a user must not take away from this table.
    if (r.error) failed += 1;
    else if (r.changed) changed += 1;
    else unchanged += 1;
  }
  return { changed, unchanged, failed };
}

/** One line of plain English for the dialog's footer. */
export function describeRename(summary: RenameSummary, applied: boolean): string {
  const parts: string[] = [];
  const verb = applied ? "renamed" : "to rename";
  parts.push(`${summary.changed} ${verb}`);
  if (summary.unchanged > 0) parts.push(`${summary.unchanged} already named correctly`);
  if (summary.failed > 0) parts.push(`${summary.failed} could not be named`);
  return parts.join(", ");
}
