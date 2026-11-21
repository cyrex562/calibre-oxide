// In-place editing of a table cell: which cells can be edited, how
// typed text becomes a set-fields change, and how a commit behaves
// (optimistic update, rollback on failure).
//
// Pure apart from `commitCellEdit`, which takes its save function as a
// parameter, so all of it is testable without a server or a DOM.

import { formatCell, type BookColumn } from "./columns";
import type { BookFieldChanges, BookSummary } from "./types";

export function isEditable(column: BookColumn): boolean {
  return column.editor !== undefined;
}

/** The first editable column's key, preferring title. */
export function defaultEditKey(columns: BookColumn[]): string | null {
  return (columns.find((c) => c.key === "title" && isEditable(c)) ?? columns.find(isEditable))?.key ?? null;
}

/** The next/previous editable column after `key`, or null at the end of the row. */
export function neighbourEditableKey(columns: BookColumn[], key: string, direction: 1 | -1): string | null {
  const at = columns.findIndex((c) => c.key === key);
  for (let i = at + direction; i >= 0 && i < columns.length; i += direction) {
    if (isEditable(columns[i])) return columns[i].key;
  }
  return null;
}

/** True when a click on the cell should start editing straight away. */
export function isEmptyCell(book: BookSummary, column: BookColumn): boolean {
  if (column.editor?.type === "bool") return book[column.key] === null || book[column.key] === undefined;
  return formatCell(book, column) === "";
}

/** The text an editor opens with. */
export function initialText(book: BookSummary, column: BookColumn): string {
  const value = book[column.key];
  if (value === null || value === undefined) return "";
  switch (column.editor?.type) {
    case "authors":
      return Array.isArray(value) ? value.join(" & ") : String(value);
    case "list":
      return Array.isArray(value) ? value.join(", ") : String(value);
    case "rating": {
      const n = Number(value);
      return Number.isFinite(n) && n > 0 ? String(n) : "";
    }
    case "date": {
      const text = formatCell(book, column) === "" ? "" : String(value).slice(0, 10);
      return /^\d{4}-\d{2}-\d{2}$/.test(text) ? text : "";
    }
    case "bool":
      return value === true || value === "true" || value === 1 ? "true" : value === false || value === "false" || value === 0 ? "false" : "";
    case "number": {
      const n = Number(value);
      return Number.isFinite(n) ? String(n) : "";
    }
    default:
      return Array.isArray(value) ? value.join(", ") : String(value);
  }
}

export type ParseResult =
  | { status: "unchanged" }
  | { status: "error"; message: string }
  | {
      status: "change";
      /** What is sent to set-fields. */
      changes: BookFieldChanges;
      /** The same change as it should read in the row, for the optimistic update. */
      patch: Partial<BookSummary>;
    };

function splitList(raw: string, separator: RegExp): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of raw.split(separator)) {
    const t = part.trim();
    if (t && !seen.has(t)) {
      seen.add(t);
      out.push(t);
    }
  }
  return out;
}

function sameList(a: unknown, b: string[]): boolean {
  return Array.isArray(a) && a.length === b.length && a.every((v, i) => v === b[i]);
}

/** Turns what was typed into a change, an error, or "nothing to do". */
export function parseCellEdit(column: BookColumn, rawInput: string, book: BookSummary): ParseResult {
  const editor = column.editor;
  if (!editor) return { status: "error", message: `${column.label} can't be edited here.` };
  const key = column.key;
  const raw = rawInput.trim();
  const current = book[key];
  const change = (value: unknown, patch: unknown = value): ParseResult => ({ status: "change", changes: { [key]: value }, patch: { [key]: patch } as Partial<BookSummary> });

  switch (editor.type) {
    case "text": {
      if (key === "title" && raw === "") return { status: "error", message: "A title can't be empty." };
      if (raw === (current == null ? "" : String(current))) return { status: "unchanged" };
      return change(raw, raw === "" ? null : raw);
    }
    case "authors": {
      const list = splitList(raw, /&/);
      if (list.length === 0) return { status: "error", message: "Enter at least one author (separate several with &)." };
      return sameList(current, list) ? { status: "unchanged" } : change(list);
    }
    case "list": {
      const list = key === "languages" ? splitList(raw.toLowerCase(), /,/) : splitList(raw, /,/);
      const same = sameList(current ?? [], list) || (current == null && list.length === 0);
      if (same) return { status: "unchanged" };
      // Plain standard fields take an array; custom columns take the
      // comma-joined string their set-field path parses.
      const standard = ["tags", "languages"].includes(key);
      return standard ? change(list) : change(list.join(", "), list);
    }
    case "number": {
      if (raw === "") {
        if (key === "series_index") return { status: "error", message: "Enter a number." };
        return current == null ? { status: "unchanged" } : change("", null);
      }
      const n = Number(raw);
      if (!Number.isFinite(n)) return { status: "error", message: `"${raw}" is not a number.` };
      if (editor.integer && !Number.isInteger(n)) return { status: "error", message: "Enter a whole number." };
      return Number(current) === n && current != null ? { status: "unchanged" } : change(n);
    }
    case "rating": {
      const starCount = (raw.match(/★/g) ?? []).length;
      let n: number;
      if (raw === "") n = 0;
      else if (starCount > 0) n = starCount + (raw.includes("½") ? 0.5 : 0);
      else n = Number(raw);
      if (!Number.isFinite(n) || n < 0 || n > 5) return { status: "error", message: "Rating is 0 to 5." };
      n = Math.round(n * 2) / 2;
      const was = Number(current ?? 0) || 0;
      return was === n ? { status: "unchanged" } : change(n, n === 0 ? null : n);
    }
    case "date": {
      const was = initialText(book, column);
      if (raw === was) return { status: "unchanged" };
      if (raw === "") return { status: "error", message: "A date can't be cleared here; pick another date." };
      const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw);
      const d = m ? new Date(`${raw}T00:00:00Z`) : null;
      if (!m || !d || Number.isNaN(d.getTime()) || d.toISOString().slice(0, 10) !== raw) return { status: "error", message: "Use a date like 2024-05-31." };
      return change(raw, `${raw}T00:00:00+00:00`);
    }
    case "enum": {
      if (raw === initialText(book, column)) return { status: "unchanged" };
      if (raw !== "" && !editor.values.includes(raw)) return { status: "error", message: `Pick one of: ${editor.values.join(", ")}.` };
      return change(raw, raw === "" ? null : raw);
    }
    case "bool": {
      if (raw === initialText(book, column)) return { status: "unchanged" };
      if (!["", "true", "false"].includes(raw)) return { status: "error", message: "Choose Yes, No or blank." };
      return change(raw, raw === "" ? null : raw === "true");
    }
  }
}

// ---------------------------------------------------------------
// Keys
// ---------------------------------------------------------------

export type EditKeyAction = "save" | "cancel" | "next" | "prev" | null;

/** What a key press means inside the open editor. */
export function editKeyAction(event: { key: string; shiftKey?: boolean; isComposing?: boolean; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean }): EditKeyAction {
  // Enter that confirms an IME composition is not "save".
  if (event.isComposing) return null;
  if (event.ctrlKey || event.metaKey || event.altKey) return null;
  switch (event.key) {
    case "Enter":
      return "save";
    case "Escape":
      return "cancel";
    case "Tab":
      return event.shiftKey ? "prev" : "next";
    default:
      return null;
  }
}

/** A key on a selected (not editing) row that opens the editor. */
export function startsEditing(event: { key: string; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean }): boolean {
  return event.key === "F2" && !event.ctrlKey && !event.metaKey && !event.altKey;
}

// ---------------------------------------------------------------
// Commit
// ---------------------------------------------------------------

/** The slice of a book window a commit needs. */
export interface EditTarget {
  getById(id: number): BookSummary | undefined;
  patch(id: number, changes: Partial<BookSummary>): BookSummary | undefined;
  put(book: BookSummary): void;
}

export type CommitResult = { status: "unchanged" } | { status: "saved"; book: BookSummary } | { status: "error"; message: string };

/**
 * Parses, applies the change to the row at once, saves it, and on
 * failure puts the row back exactly as it was.
 */
export async function commitCellEdit(args: { target: EditTarget; bookId: number; column: BookColumn; raw: string; save: (bookId: number, changes: BookFieldChanges) => Promise<BookSummary> }): Promise<CommitResult> {
  const { target, bookId, column, raw, save } = args;
  const book = target.getById(bookId);
  if (!book) return { status: "error", message: "That book is no longer loaded." };
  const parsed = parseCellEdit(column, raw, book);
  if (parsed.status === "unchanged") return parsed;
  if (parsed.status === "error") return parsed;

  const before = target.patch(bookId, parsed.patch);
  try {
    const saved = await save(bookId, parsed.changes);
    if (saved) target.put(saved);
    return { status: "saved", book: saved ?? target.getById(bookId)! };
  } catch (e) {
    if (before) target.put(before);
    return { status: "error", message: e instanceof Error ? e.message : String(e) };
  }
}
