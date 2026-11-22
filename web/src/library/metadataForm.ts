// The details panel's always-on metadata form: building it from a book,
// telling whether it was changed, and turning it into a save request.
//
// Pure so the dirty / save / revert rules are tested directly.

import type { BookFieldChanges, BookSummary, FieldMetaEntry } from "./types";

export interface MetaForm {
  title: string;
  authors: string;
  series: string;
  seriesIndex: string;
  tags: string;
  rating: number;
  publisher: string;
  pubdate: string;
  comments: string;
  /** Custom column values keyed by the column's bare label, as strings. */
  custom: Record<string, string>;
}

/** Fills the form from a book row. */
export function formFromBook(b: BookSummary, customFields: FieldMetaEntry[]): MetaForm {
  const custom: Record<string, string> = {};
  for (const field of customFields) {
    const raw = b[field.label];
    // A bool custom column reads back as "0"/"1" (an integer column
    // under the hood), never "true"/"false".
    if (field.datatype === "bool") custom[field.label] = raw === "1" || raw === "true" || raw === true ? "true" : "false";
    else custom[field.label] = raw == null ? "" : String(raw);
  }
  return {
    title: b.title,
    authors: (b.authors ?? []).join(" & "),
    series: b.series ?? "",
    seriesIndex: b.series_index != null ? String(b.series_index) : "",
    tags: (b.tags ?? []).join(", "),
    rating: b.rating ?? 0,
    publisher: (b.publisher as string | null) ?? "",
    // The server sends an ISO timestamp; `<input type="date">` wants
    // just the date part and rejects the whole string.
    pubdate: typeof b.pubdate === "string" ? b.pubdate.slice(0, 10) : "",
    comments: (b.comments as string | null) ?? "",
    custom,
  };
}

/** A copy of a form. Not `structuredClone`: the form is usually a reactive proxy, which that cannot clone. */
export function cloneForm(form: MetaForm): MetaForm {
  return { ...form, custom: { ...form.custom } };
}

/** True when `form` differs from the `baseline` it was filled from. */
export function isFormDirty(form: MetaForm, baseline: MetaForm): boolean {
  const { custom: c1, ...rest1 } = form;
  const { custom: c2, ...rest2 } = baseline;
  if (JSON.stringify(rest1) !== JSON.stringify(rest2)) return true;
  const keys = new Set([...Object.keys(c1), ...Object.keys(c2)]);
  for (const k of keys) if ((c1[k] ?? "") !== (c2[k] ?? "")) return true;
  return false;
}

/** The `set-fields` body for a form: the same one the old edit mode sent. */
export function changesFromForm(form: MetaForm, customFields: FieldMetaEntry[]): BookFieldChanges {
  const changes: BookFieldChanges = {
    title: form.title,
    authors: form.authors.split("&").map((a) => a.trim()).filter(Boolean),
    series: form.series,
    tags: form.tags.split(",").map((t) => t.trim()).filter(Boolean),
    rating: form.rating,
    publisher: form.publisher,
    comments: form.comments,
  };
  // Sent only when set: an empty string is not a date, and clearing
  // the field should leave the stored value alone.
  if (form.pubdate.trim() !== "") changes.pubdate = form.pubdate;
  if (form.seriesIndex.trim() !== "") {
    const idx = Number(form.seriesIndex);
    if (!Number.isNaN(idx)) changes.series_index = idx;
  }
  for (const field of customFields) {
    if (!field.is_editable) continue;
    changes[field.label] = form.custom[field.label] ?? "";
  }
  return changes;
}

/**
 * Per-book drafts: unsaved edits are kept when the user moves to another
 * book and restored when they come back, rather than being discarded
 * silently or interrupting an arrow-key walk down the list with a prompt.
 */
export class DraftStore {
  private drafts = new Map<number, MetaForm>();

  /** Stores the form if it is dirty, forgets any draft if it is not. */
  stash(bookId: number, form: MetaForm, baseline: MetaForm): void {
    if (isFormDirty(form, baseline)) this.drafts.set(bookId, cloneForm(form));
    else this.drafts.delete(bookId);
  }

  take(bookId: number): MetaForm | null {
    return this.drafts.get(bookId) ?? null;
  }

  discard(bookId: number): void {
    this.drafts.delete(bookId);
  }

  has(bookId: number): boolean {
    return this.drafts.has(bookId);
  }
}
