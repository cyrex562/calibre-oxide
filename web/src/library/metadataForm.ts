// The details panel's always-on metadata form: building it from a book,
// telling whether it was changed, and turning it into a save request.
//
// Pure so the dirty / save / revert rules are tested directly.

import { ref } from "vue";
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
  // A number input hands back a number for what was filled in as a string.
  if (JSON.stringify({ ...rest1, seriesIndex: String(rest1.seriesIndex ?? "") }) !== JSON.stringify({ ...rest2, seriesIndex: String(rest2.seriesIndex ?? "") })) return true;
  const keys = new Set([...Object.keys(c1), ...Object.keys(c2)]);
  for (const k of keys) if (String(c1[k] ?? "") !== String(c2[k] ?? "")) return true;
  return false;
}

/** The `set-fields` body for a form: the same one the old edit mode sent. */
export function changesFromForm(form: MetaForm, customFields: FieldMetaEntry[], baseline?: MetaForm | null): BookFieldChanges {
  // With a baseline only the fields the user actually edited are sent, so
  // a save cannot revert a change made elsewhere (a table cell, say).
  const touched = (key: Exclude<keyof MetaForm, "custom">): boolean =>
    !baseline || String(form[key] ?? "") !== String(baseline[key] ?? "");
  const changes: BookFieldChanges = {};
  if (touched("title")) changes.title = form.title;
  if (touched("authors")) changes.authors = form.authors.split("&").map((a) => a.trim()).filter(Boolean);
  if (touched("series")) changes.series = form.series;
  if (touched("tags")) changes.tags = form.tags.split(",").map((t) => t.trim()).filter(Boolean);
  if (touched("rating")) changes.rating = form.rating;
  if (touched("publisher")) changes.publisher = form.publisher;
  if (touched("comments")) changes.comments = form.comments;
  // Sent only when set: an empty string is not a date, and clearing
  // the field should leave the stored value alone.
  if (touched("pubdate") && form.pubdate.trim() !== "") changes.pubdate = form.pubdate;
  if (touched("seriesIndex") && String(form.seriesIndex ?? "").trim() !== "") {
    const idx = Number(form.seriesIndex);
    if (!Number.isNaN(idx)) changes.series_index = idx;
  }
  for (const field of customFields) {
    if (!field.is_editable) continue;
    const value = String(form.custom[field.label] ?? "");
    if (baseline && value === String(baseline.custom[field.label] ?? "")) continue;
    changes[field.label] = value;
  }
  return changes;
}

/**
 * The form after the book was re-read: fields the user left alone take
 * the new values, edited fields keep what was typed.
 */
export function rebaseForm(form: MetaForm, oldBaseline: MetaForm, newBaseline: MetaForm): MetaForm {
  const out = cloneForm(newBaseline) as unknown as Record<string, unknown>;
  const f = form as unknown as Record<string, unknown>;
  const o = oldBaseline as unknown as Record<string, unknown>;
  for (const key of Object.keys(newBaseline)) {
    if (key === "custom") continue;
    if (String(f[key] ?? "") !== String(o[key] ?? "")) out[key] = f[key];
  }
  const custom = { ...newBaseline.custom };
  for (const key of Object.keys(form.custom)) {
    if (String(form.custom[key] ?? "") !== String(oldBaseline.custom[key] ?? "")) custom[key] = form.custom[key];
  }
  out.custom = custom;
  return out as unknown as MetaForm;
}

/**
 * Per-book drafts: unsaved edits are kept when the user moves to another
 * book and restored when they come back, rather than being discarded
 * silently or interrupting an arrow-key walk down the list with a prompt.
 */
export class DraftStore {
  private drafts = new Map<number, MetaForm>();
  private bases = new Map<number, MetaForm>();

  /** Stores the form if it is dirty, forgets any draft if it is not. */
  stash(bookId: number, form: MetaForm, baseline: MetaForm): void {
    if (isFormDirty(form, baseline)) {
      this.drafts.set(bookId, cloneForm(form));
      this.bases.set(bookId, cloneForm(baseline));
    } else {
      this.drafts.delete(bookId);
      this.bases.delete(bookId);
    }
  }

  take(bookId: number): MetaForm | null {
    return this.drafts.get(bookId) ?? null;
  }

  /**
   * The draft rebased onto the book's current values: fields the draft
   * did not touch follow edits made elsewhere since it was stashed.
   */
  restore(bookId: number, fresh: MetaForm): MetaForm | null {
    const draft = this.drafts.get(bookId);
    if (!draft) return null;
    const base = this.bases.get(bookId);
    return base ? rebaseForm(draft, base, fresh) : cloneForm(draft);
  }

  discard(bookId: number): void {
    this.drafts.delete(bookId);
    this.bases.delete(bookId);
  }

  has(bookId: number): boolean {
    return this.drafts.has(bookId);
  }

  /** How many books have an unsaved draft. */
  get size(): number {
    return this.drafts.size;
  }

  ids(): number[] {
    return [...this.drafts.keys()];
  }

  clear(): void {
    this.drafts.clear();
    this.bases.clear();
  }
}

/**
 * The drafts for the whole library window. Module-level, so collapsing
 * the details panel, opening the reader or deleting a book does not lose
 * them; `draftCount` is reactive for the "N unsaved drafts" indicator.
 */
class SharedDraftStore extends DraftStore {
  readonly count = ref(0);
  override stash(bookId: number, form: MetaForm, baseline: MetaForm): void {
    super.stash(bookId, form, baseline);
    this.count.value = this.size;
  }
  override discard(bookId: number): void {
    super.discard(bookId);
    this.count.value = this.size;
  }
  override clear(): void {
    super.clear();
    this.count.value = 0;
  }
}
export const sharedDrafts = new SharedDraftStore();
export const draftCount = sharedDrafts.count;

