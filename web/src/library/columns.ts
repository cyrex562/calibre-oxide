// Column model for the library table view (issue 1.1 of the #816 epic).
//
// # Why a table at all
//
// The cover grid was the only way to see a library. For ebooks that is
// fine; for a collection of PDFs -- reports, papers, scans -- covers
// are usually blank or identical, and the grid degrades into a wall of
// grey rectangles. A sortable column table is what upstream calibre
// makes its primary view, and it is the interface that actually
// supports *managing* a collection rather than browsing one.
//
// # This module is pure
//
// Everything here is data and pure functions over a book row, so the
// rules that decide what a cell shows can be tested directly. The
// component does rendering, resizing and event handling only.
//
// # Field keys come from the server, not from guesswork
//
// `GET /ajax/field-metadata` returns both the field catalogue and
// `sortable_fields`. Those two sets are *not* the same, which is the
// trap this module exists to avoid: `size`, `ondevice` and `cover` are
// all sortable but never appear in a book row, so a column list
// derived from `sortable_fields` would render three permanently-empty
// columns. `columnsFor` intersects the two instead.

import type { BookSummary, FieldMetaEntry } from "./types";

/** How a cell's value should be turned into text. */
export type ColumnKind = "text" | "list" | "date" | "rating" | "formats" | "number";

export interface BookColumn {
  /** Key into the book row. */
  key: string;
  label: string;
  kind: ColumnKind;
  /**
   * Sort key for `/ajax/search`, when the server will sort by this
   * field. Absent means the column is displayable but not sortable.
   */
  sortKey?: string;
  /** Column width in pixels. */
  width: number;
}

/**
 * Columns shown to someone who has never configured any, in order.
 *
 * Deliberately not "everything": a first run should be readable, and
 * the rest are one click away in the column picker.
 */
export const DEFAULT_VISIBLE_KEYS = ["title", "authors", "series", "tags", "publisher", "timestamp", "formats"];

/** Per-field display rules, keyed by the book row's own key. */
const KNOWN_COLUMNS: Record<string, { label: string; kind: ColumnKind; width: number }> = {
  title: { label: "Title", kind: "text", width: 320 },
  authors: { label: "Authors", kind: "list", width: 200 },
  author_sort: { label: "Author sort", kind: "text", width: 200 },
  series: { label: "Series", kind: "text", width: 160 },
  series_index: { label: "#", kind: "number", width: 60 },
  tags: { label: "Tags", kind: "list", width: 180 },
  publisher: { label: "Publisher", kind: "text", width: 160 },
  rating: { label: "Rating", kind: "rating", width: 110 },
  pubdate: { label: "Published", kind: "date", width: 120 },
  timestamp: { label: "Date added", kind: "date", width: 120 },
  last_modified: { label: "Modified", kind: "date", width: 120 },
  languages: { label: "Languages", kind: "list", width: 110 },
  formats: { label: "Formats", kind: "formats", width: 120 },
  identifiers: { label: "Identifiers", kind: "text", width: 160 },
  isbn: { label: "ISBN", kind: "text", width: 140 },
  comments: { label: "Comments", kind: "text", width: 260 },
  id: { label: "ID", kind: "number", width: 70 },
  uuid: { label: "UUID", kind: "text", width: 260 },
};

/**
 * Row keys that exist but must never become columns: internal
 * plumbing, or values that are URLs rather than anything a person
 * would read in a cell.
 */
const NEVER_A_COLUMN = new Set(["cover", "thumbnail", "main_format", "other_formats", "available_formats", "sort"]);

/** Maps a custom column's `datatype` onto how its cell should render. */
function kindForDatatype(datatype: string | null): ColumnKind {
  switch (datatype) {
    case "rating":
      return "rating";
    case "datetime":
      return "date";
    case "int":
    case "float":
      return "number";
    case "text":
      // Custom `text` columns are multi-valued when they hold tag-like
      // data; the formatter handles both, so "list" is the safe read.
      return "list";
    default:
      return "text";
  }
}

/**
 * Every column that can be shown for this library, in a stable order:
 * the known standard fields first (in `KNOWN_COLUMNS` order), then any
 * custom columns the library defines.
 *
 * `sortable` is the `sortable_fields` list from the server. A field
 * missing from it still gets a column -- it just cannot be sorted by.
 */
export function columnsFor(fieldMetadata: Record<string, FieldMetaEntry>, sortable: [string, string][]): BookColumn[] {
  const sortableKeys = new Set(sortable.map(([k]) => k));
  const columns: BookColumn[] = [];

  for (const [key, spec] of Object.entries(KNOWN_COLUMNS)) {
    columns.push({ key, ...spec, ...(sortableKeys.has(key) ? { sortKey: key } : {}) });
  }

  // Custom columns arrive as `#label` keys in the field metadata; the
  // book row stores them under the bare `label`.
  for (const entry of Object.values(fieldMetadata)) {
    if (!entry.is_custom) continue;
    const key = entry.label;
    if (!key || NEVER_A_COLUMN.has(key) || columns.some((c) => c.key === key)) continue;
    columns.push({
      key,
      label: entry.name || key,
      kind: kindForDatatype(entry.datatype),
      width: 140,
      ...(sortableKeys.has(entry.key) ? { sortKey: entry.key } : {}),
    });
  }

  return columns;
}

/**
 * Calibre stores "no date" as year 101, which naively formats as
 * "1/1/101" and looks like corrupt data in a column of real dates.
 */
function isUndefinedDate(d: Date): boolean {
  return d.getUTCFullYear() <= 101;
}

/**
 * The text for one cell. Returns "" for anything absent, so an empty
 * cell is always genuinely empty rather than "null" or "undefined".
 */
export function formatCell(book: BookSummary, column: BookColumn): string {
  const value = book[column.key];
  if (value === null || value === undefined) return "";

  switch (column.kind) {
    case "list": {
      if (Array.isArray(value)) return value.filter((v) => v !== null && v !== undefined && v !== "").join(column.key === "authors" ? " & " : ", ");
      return String(value);
    }
    case "formats": {
      if (!Array.isArray(value)) return String(value).toUpperCase();
      return value.map((f) => String(f).toUpperCase()).join(", ");
    }
    case "date": {
      const d = new Date(String(value));
      if (Number.isNaN(d.getTime()) || isUndefinedDate(d)) return "";
      return d.toLocaleDateString();
    }
    case "rating": {
      const n = Number(value);
      if (!Number.isFinite(n) || n <= 0) return "";
      // `ajax::book_json` already halves calibre's 0..10 to 0..5.
      const whole = Math.floor(n);
      return "★".repeat(whole) + (n - whole >= 0.5 ? "½" : "");
    }
    case "number": {
      const n = Number(value);
      return Number.isFinite(n) ? String(n) : "";
    }
    case "text":
    default: {
      if (Array.isArray(value)) return value.join(", ");
      if (typeof value === "object") return "";
      return String(value);
    }
  }
}

// ---------------------------------------------------------------
// Persisted preferences
// ---------------------------------------------------------------

export const TABLE_PREFS_PROFILE = "table-prefs";

export type LibraryViewMode = "table" | "grid";

export interface TablePrefs {
  /**
   * Which view the library opens in. Defaults to the table: this tool
   * is aimed at PDF collections, where covers carry little
   * information.
   */
  view: LibraryViewMode;
  /** Visible column keys, in display order. */
  columns: string[];
  /** Per-column width overrides, keyed by column key. */
  widths: Record<string, number>;
}

export const DEFAULT_TABLE_PREFS: TablePrefs = { view: "table", columns: [...DEFAULT_VISIBLE_KEYS], widths: {} };

/** Narrower than this and a column is unreadable; wider is unusable. */
export const MIN_COLUMN_WIDTH = 60;
export const MAX_COLUMN_WIDTH = 900;

export function clampWidth(width: number): number {
  if (!Number.isFinite(width)) return MIN_COLUMN_WIDTH;
  return Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, Math.round(width)));
}

/**
 * The columns to actually render: the user's chosen keys, in their
 * chosen order, resolved against what this library really offers.
 *
 * Keys the library no longer has (a deleted custom column) drop out
 * silently rather than rendering a permanently-empty column. If that
 * would leave nothing at all, the defaults come back -- a table with
 * no columns is not a view, it is a bug the user cannot escape from
 * without clearing their preferences by hand.
 */
export function resolveColumns(available: BookColumn[], prefs: TablePrefs): BookColumn[] {
  const byKey = new Map(available.map((c) => [c.key, c]));
  const chosen = prefs.columns.map((key) => byKey.get(key)).filter((c): c is BookColumn => c !== undefined);
  const resolved = chosen.length > 0 ? chosen : DEFAULT_VISIBLE_KEYS.map((k) => byKey.get(k)).filter((c): c is BookColumn => c !== undefined);
  return resolved.map((c) => ({ ...c, width: clampWidth(prefs.widths[c.key] ?? c.width) }));
}

/**
 * The sort state a header click should produce.
 *
 * Clicking the column already sorted by flips direction; clicking any
 * other column sorts by it, starting ascending -- except for dates,
 * where newest-first is what someone actually wants from a first
 * click on "Date added".
 */
export function nextSort(column: BookColumn, currentSort: string, currentOrder: "asc" | "desc"): { sort: string; order: "asc" | "desc" } | null {
  if (!column.sortKey) return null;
  const primary = currentSort.split(",")[0]?.trim();
  if (primary === column.sortKey) {
    return { sort: currentSort, order: currentOrder === "asc" ? "desc" : "asc" };
  }
  return { sort: column.sortKey, order: column.kind === "date" ? "desc" : "asc" };
}
