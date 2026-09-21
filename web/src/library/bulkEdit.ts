// Bulk metadata editing (issue 1.6 of the #816 epic).
//
// The previous bulk edit reached three fields -- add tags, remove
// tags, set series -- plus a rating. `Cache::set_field` accepts rather
// more than that: authors, publisher, languages, pubdate, comments,
// series_index and any custom column, all of which are exactly the
// things you need when tidying an imported pile of PDFs.
//
// # Why this is pure
//
// Computing the change set is where the bugs live: removing a tag
// that was never there, re-adding one that already exists, a
// search-and-replace that silently matches nothing, a regex the user
// typed that does not compile. None of that is visible in a component
// until it has already written the wrong thing to a hundred books, so
// it lives here where it can be stated and tested.

import type { BookFieldChanges, BookSummary } from "./types";

/** How a multi-valued field (tags, authors, languages) is changed. */
export type ListMode = "add" | "remove" | "replace";

export interface ListEdit {
  mode: ListMode;
  /** Comma-separated user input. */
  value: string;
}

/** Fields a search-and-replace can be run over. */
export const REPLACEABLE_FIELDS = ["title", "authors", "series", "publisher", "tags", "comments"] as const;
export type ReplaceableField = (typeof REPLACEABLE_FIELDS)[number];

export interface SearchReplace {
  field: ReplaceableField;
  find: string;
  replace: string;
  useRegex: boolean;
}

export interface BulkEditSpec {
  tags?: ListEdit;
  authors?: ListEdit;
  languages?: ListEdit;
  /** Single-valued fields. An empty string means "leave alone". */
  publisher?: string;
  series?: string;
  seriesIndex?: number | null;
  rating?: number | null;
  /** ISO date string, or "" to leave alone. */
  pubdate?: string;
  comments?: string;
  /** Custom column values, keyed by the bare label a book row uses. */
  custom?: Record<string, string>;
  searchReplace?: SearchReplace;
}

/** Splits comma-separated user input into clean values. */
export function parseList(value: string): string[] {
  return value
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);
}

/**
 * Applies a list edit to a book's current values.
 *
 * Order within the result is stable and duplicates are collapsed:
 * "add" on a value already present must be a no-op, not a second copy.
 * Removal is case-insensitive, because a user typing "scifi" to remove
 * "SciFi" means the same tag -- and nothing in the UI would tell them
 * why it had not worked.
 */
export function applyListEdit(current: string[], edit: ListEdit): string[] {
  const values = parseList(edit.value);
  if (values.length === 0 && edit.mode !== "replace") return current;

  switch (edit.mode) {
    case "replace":
      return dedupe(values);
    case "add":
      return dedupe([...current, ...values]);
    case "remove": {
      const drop = new Set(values.map((v) => v.toLowerCase()));
      return current.filter((v) => !drop.has(v.toLowerCase()));
    }
  }
}

function dedupe(values: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const v of values) {
    const k = v.toLowerCase();
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(v);
  }
  return out;
}

/** Thrown for a regex the user typed that does not compile. */
export class InvalidPatternError extends Error {}

/**
 * Runs a search-and-replace over one text value.
 *
 * Plain (non-regex) mode never builds a regex at all -- it splits on
 * the literal text and rejoins -- so a user searching for "C++" or
 * "(draft)" gets what they asked for rather than a regex error or a
 * wildly wrong match. Both modes replace every occurrence.
 */
export function replaceIn(value: string, sr: Pick<SearchReplace, "find" | "replace" | "useRegex">): string {
  if (!sr.find) return value;
  if (!sr.useRegex) return value.split(sr.find).join(sr.replace);

  let re: RegExp;
  try {
    re = new RegExp(sr.find, "g");
  } catch (e) {
    throw new InvalidPatternError(e instanceof Error ? e.message : String(e));
  }
  return value.replace(re, sr.replace);
}

/** Reads a field off a book row as the list or scalar it really is. */
function currentList(book: BookSummary, key: string): string[] {
  const v = book[key];
  if (Array.isArray(v)) return v.map(String);
  if (typeof v === "string" && v) return [v];
  return [];
}

function currentText(book: BookSummary, key: string): string {
  const v = book[key];
  if (Array.isArray(v)) return v.join(", ");
  if (v === null || v === undefined) return "";
  return String(v);
}

/**
 * The changes to send for one book, or `null` if this book needs no
 * write at all.
 *
 * Returning `null` matters: a bulk edit over hundreds of books should
 * not POST for every one of them when only a few actually differ, and
 * every needless write bumps `last_modified`.
 *
 * Throws `InvalidPatternError` for a bad regex -- the caller is
 * expected to validate once up front rather than per book.
 */
export function changesFor(book: BookSummary, spec: BulkEditSpec): BookFieldChanges | null {
  const changes: BookFieldChanges = {};

  const lists: [keyof BulkEditSpec & ("tags" | "authors" | "languages"), string][] = [
    ["tags", "tags"],
    ["authors", "authors"],
    ["languages", "languages"],
  ];
  for (const [specKey, rowKey] of lists) {
    const edit = spec[specKey];
    if (!edit) continue;
    const current = currentList(book, rowKey);
    const next = applyListEdit(current, edit);
    if (!sameList(current, next)) changes[rowKey] = next;
  }

  if (spec.publisher) changes.publisher = spec.publisher;
  if (spec.series) changes.series = spec.series;
  if (spec.seriesIndex !== null && spec.seriesIndex !== undefined) changes.series_index = spec.seriesIndex;
  if (spec.rating !== null && spec.rating !== undefined) changes.rating = spec.rating;
  if (spec.pubdate) changes.pubdate = spec.pubdate;
  if (spec.comments) changes.comments = spec.comments;

  for (const [key, value] of Object.entries(spec.custom ?? {})) {
    if (value !== "") changes[key] = value;
  }

  if (spec.searchReplace?.find) {
    const sr = spec.searchReplace;
    // Run against whatever the earlier edits already produced, so a
    // replace composes with them instead of fighting over the field.
    const before = sr.field in changes ? stringifyChange(changes[sr.field]) : currentText(book, sr.field);
    const after = replaceIn(before, sr);
    if (after !== before) {
      // `authors` and `tags` are multi-valued; the replace runs over
      // the joined text, so the result has to be split back apart or
      // the whole list collapses into a single value.
      // Widened because `sr.field` is a union: indexing `changes`
      // with it makes TypeScript demand a value assignable to *every*
      // member's type at once (`string & string[]`), which nothing
      // satisfies. `BookFieldChanges` carries an index signature, so
      // this stays type-safe at the point of use.
      (changes as Record<string, unknown>)[sr.field] = sr.field === "authors" || sr.field === "tags" ? parseList(after) : after;
    }
  }

  return Object.keys(changes).length > 0 ? changes : null;
}

function stringifyChange(value: unknown): string {
  if (Array.isArray(value)) return value.join(", ");
  return value === null || value === undefined ? "" : String(value);
}

function sameList(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/**
 * Validates a spec once, before touching any book.
 *
 * Returns an error message, or `null` when the spec is usable.
 */
export function validateSpec(spec: BulkEditSpec): string | null {
  const sr = spec.searchReplace;
  if (sr?.find && sr.useRegex) {
    try {
      new RegExp(sr.find);
    } catch (e) {
      return `Invalid search pattern: ${e instanceof Error ? e.message : String(e)}`;
    }
  }
  return null;
}

/** Whether the spec would change anything at all. */
export function isEmptySpec(spec: BulkEditSpec): boolean {
  const hasList = (e?: ListEdit) => !!e && (e.mode === "replace" || parseList(e.value).length > 0);
  return !(
    hasList(spec.tags) ||
    hasList(spec.authors) ||
    hasList(spec.languages) ||
    !!spec.publisher ||
    !!spec.series ||
    spec.seriesIndex !== null ||
    spec.rating !== null ||
    !!spec.pubdate ||
    !!spec.comments ||
    Object.values(spec.custom ?? {}).some((v) => v !== "") ||
    !!spec.searchReplace?.find
  );
}
