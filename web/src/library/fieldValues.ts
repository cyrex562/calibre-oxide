// Names the library already knows (authors, publishers, tags, series,
// custom-column values), offered as suggestions while typing.
//
// Three small pieces, all independent of the DOM:
//  - a per-field cache of known values ordered by how many books use
//    them (fetched once, shared, dropped when a save may have changed them);
//  - `suggest` / `applySuggestion`, the pure matching and completion rules;
//  - `completionFor`, the one mapping from a column to what it completes.

import { fetchCategoryValues } from "./api";
import type { BookColumn } from "./columns";
import type { BookFieldChanges, FieldMetaEntry } from "./types";
import { getItemCompat } from "./legacyStorage";

export interface FieldValue {
  name: string;
  /** How many books use the value; 0 when unknown (a fixed list). */
  count: number;
}

export interface Suggestion extends FieldValue {
  /** The matched part of `name` as [start, end), or null for an acronym match. */
  range: [number, number] | null;
}

/** What a field completes: its category name and how several values are separated. */
export interface Completion {
  field: string;
  separator: "&" | "," | null;
}

// ---------------------------------------------------------------
// Which columns complete, and against what
// ---------------------------------------------------------------

const STANDARD: Record<string, Completion> = {
  authors: { field: "authors", separator: "&" },
  tags: { field: "tags", separator: "," },
  series: { field: "series", separator: null },
  publisher: { field: "publisher", separator: null },
};

/** The completion for a standard field key (`authors`, `tags`, `series`, `publisher`), or null. */
export function completionForStandard(key: string): Completion | null {
  return STANDARD[key] ?? null;
}

/** The completion for a custom column's field metadata (text, series; multi-value text splits on commas). Enumerations are handled by their own value list. */
export function completionForCustom(entry: FieldMetaEntry): Completion | null {
  if (!entry.is_custom || entry.is_editable === false) return null;
  if (entry.datatype === "text") return { field: entry.key, separator: entry.is_multiple ? "," : null };
  if (entry.datatype === "series") return { field: entry.key, separator: null };
  return null;
}

/** The fixed values of an enumeration custom column, or null when it is not one. */
export function enumValuesOf(entry: FieldMetaEntry): string[] | null {
  if (entry.datatype !== "enumeration") return null;
  const display = entry.display as { enum_values?: unknown } | undefined;
  return Array.isArray(display?.enum_values) ? display.enum_values.map(String) : null;
}

/** What a table column completes, or null. Used by the cell editor. */
export function completionFor(column: BookColumn): Completion | null {
  const editor = column.editor;
  if (!editor) return null;
  if (column.field) {
    // A custom column: its category is `#label`.
    if (editor.type === "text") return { field: column.field, separator: null };
    if (editor.type === "list") return { field: column.field, separator: "," };
    return null;
  }
  if (editor.type === "authors" || editor.type === "list" || editor.type === "text") return STANDARD[column.key] ?? null;
  return null;
}

// ---------------------------------------------------------------
// Matching
// ---------------------------------------------------------------

export const MIN_CHARS_DEFAULT = 2;
export const MIN_CHARS_RANGE: [number, number] = [1, 5];
export const MIN_CHARS_KEY = "lexmechanic.suggestMinChars";

export function clampMinChars(n: number): number {
  if (!Number.isFinite(n)) return MIN_CHARS_DEFAULT;
  return Math.min(MIN_CHARS_RANGE[1], Math.max(MIN_CHARS_RANGE[0], Math.round(n)));
}

/** The "Suggest after N characters" setting. Read on every use so a change applies at once. */
export function getMinChars(): number {
  try {
    const raw = getItemCompat(localStorage, MIN_CHARS_KEY);
    if (raw === null) return MIN_CHARS_DEFAULT;
    return clampMinChars(Number(raw));
  } catch {
    return MIN_CHARS_DEFAULT;
  }
}

export function setMinChars(n: number): number {
  const v = clampMinChars(n);
  try {
    localStorage.setItem(MIN_CHARS_KEY, String(v));
  } catch {
    /* storage unavailable: the default applies */
  }
  return v;
}

const EXTRA_FOLD: Record<string, string> = { "ł": "l", "Ł": "l", "ø": "o", "Ø": "o", "đ": "d", "Đ": "d", "ı": "i" };

/**
 * Lower-cased, accent-stripped text, with `map[i]` giving the index in
 * the original string that normalized character `i` came from (so a
 * match can be highlighted in the original).
 */
function fold(text: string): { s: string; map: number[] } {
  let s = "";
  const map: number[] = [];
  let at = 0;
  for (const ch of text) {
    const out = (EXTRA_FOLD[ch] ?? ch.normalize("NFD").replace(/\p{M}/gu, "")).toLowerCase();
    for (let i = 0; i < out.length; i++) {
      s += out[i];
      map.push(at);
    }
    at += ch.length;
  }
  map.push(at);
  return { s, map };
}

const isWordChar = (c: string) => /[\p{L}\p{N}]/u.test(c);

interface Folded {
  s: string;
  map: number[];
  /** First letters of the words, for matching "wwn" against "Worlds Without Number". */
  initials: string;
}

/**
 * A value's folded form, computed once per value object. `suggest` runs on
 * every keystroke over every known value (tens of thousands of authors);
 * folding each of them each time was most of its cost.
 */
const foldedCache = new WeakMap<FieldValue, Folded>();

function foldedOf(v: FieldValue): Folded {
  let f = foldedCache.get(v);
  if (!f) {
    const { s, map } = fold(v.name);
    f = { s, map, initials: s.split(/[^\p{L}\p{N}]+/u).filter(Boolean).map((w) => w[0]).join("") };
    foldedCache.set(v, f);
  }
  return f;
}

/** Splits text into the segments before the last separator and the one being typed. */
export function splitSegments(text: string, separator: string | null): { before: string[]; current: string } {
  if (!separator) return { before: [], current: text };
  const parts = text.split(separator);
  const current = parts.pop() ?? "";
  return { before: parts.map((p) => p.trim()).filter(Boolean), current };
}

export interface SuggestOptions {
  separator?: string | null;
  minChars?: number;
  limit?: number;
}

/**
 * Suggestions for what is being typed.
 *
 * Only the segment after the last separator is completed, and values
 * already entered in the other segments are left out. Matching ignores
 * case and accents. Ranking, best first: starts with the text, a word
 * starts with it ("number" -> "Worlds Without Number"), the words'
 * initials start with it ("wwn"), then contains it. Within a rank the
 * most-used value comes first. A value typed exactly as it already is
 * is not offered back.
 */
export function suggest(values: FieldValue[], text: string, opts: SuggestOptions = {}): Suggestion[] {
  const { separator = null, minChars = MIN_CHARS_DEFAULT, limit = 10 } = opts;
  const { before, current } = splitSegments(text, separator);
  const typed = current.trim();
  const q = fold(typed).s;
  if ([...typed].length < minChars || q === "") return [];

  const used = new Set(before.map((b) => fold(b).s));
  const compact = !/\s/.test(q);
  const hits: { v: FieldValue; rank: number; range: [number, number] | null }[] = [];

  for (const v of values) {
    if (v.name === typed) continue;
    const { s, map, initials } = foldedOf(v);
    if (used.has(s)) continue;
    const range = (from: number): [number, number] => [map[from], map[from + q.length]];
    let rank = -1;
    let hit: [number, number] | null = null;
    if (s.startsWith(q)) {
      rank = 0;
      hit = range(0);
    } else {
      for (let i = 1; i < s.length; i++) {
        if (!isWordChar(s[i - 1]) && isWordChar(s[i]) && s.startsWith(q, i)) {
          rank = 1;
          hit = range(i);
          break;
        }
      }
      if (rank < 0 && compact) {
        if (initials.startsWith(q)) rank = 2;
      }
      if (rank < 0) {
        const i = s.indexOf(q);
        if (i >= 0) {
          rank = 3;
          hit = range(i);
        }
      }
    }
    if (rank >= 0) hits.push({ v, rank, range: hit });
  }

  hits.sort((a, b) => a.rank - b.rank || b.v.count - a.v.count || a.v.name.localeCompare(b.v.name));
  return hits.slice(0, limit).map((h) => ({ name: h.v.name, count: h.v.count, range: h.range }));
}

/**
 * Puts a chosen value in place of the segment being typed. Several-value
 * fields come back as `A & B & ` / `a, b, ` (spaced, with the separator
 * ready for the next name); single-value fields as just the value.
 */
export function applySuggestion(text: string, value: string, separator: string | null): string {
  if (!separator) return value;
  const { before } = splitSegments(text, separator);
  const glue = separator === "&" ? " & " : ", ";
  return [...before, value].join(glue) + glue;
}

// ---------------------------------------------------------------
// Cache
// ---------------------------------------------------------------

type Loader = (field: string) => Promise<FieldValue[]>;

const cache = new Map<string, Promise<FieldValue[]>>();

async function load(field: string): Promise<FieldValue[]> {
  const page = await fetchCategoryValues(field);
  const values = page.items.map((i) => ({ name: i.name, count: i.count }));
  // Folded now, once, so the first keystroke does not pay for all of them.
  for (const v of values) foldedOf(v);
  return values;
}

/**
 * The known values of a field, most-used first. Fetched on first use;
 * concurrent callers share the one request. A failed fetch gives an
 * empty list (suggestions are a convenience) and is retried next time.
 */
export function getFieldValues(field: string, loader: Loader = load): Promise<FieldValue[]> {
  let p = cache.get(field);
  if (!p) {
    const mine: Promise<FieldValue[]> = loader(field).catch(() => {
      if (cache.get(field) === mine) cache.delete(field);
      return [];
    });
    p = mine;
    cache.set(field, p);
  }
  return p;
}

/** Forgets one field's values (or all of them) so the next use refetches. Call after anything that may have added or removed names. */
export function invalidateFieldValues(field?: string): void {
  if (field === undefined) cache.clear();
  else cache.delete(field);
}

/** Invalidates the fields a save touched (custom columns are keyed `#label`). */
export function invalidateForChanges(changes: BookFieldChanges): void {
  for (const key of Object.keys(changes)) invalidateFieldValues(STANDARD[key] ? key : `#${key}`);
}
