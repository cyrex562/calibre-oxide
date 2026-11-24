// One-click custom-column presets for organizing a collection: edition, game
// line / system, volume. They use the column types that already exist; a preset
// only fills in a label, a display name and a datatype, and the name is
// editable before it is created.

import * as realApi from "./api";

export interface ColumnPreset {
  id: string;
  title: string;
  label: string;
  name: string;
  datatype: "text" | "int";
  /** What using it looks like, shown with the preset. */
  hint: string;
}

export const COLUMN_PRESETS: ColumnPreset[] = [
  { id: "edition", title: "Edition", label: "edition", name: "Edition", datatype: "text", hint: "Text, like “2nd edition” or “Revised”." },
  { id: "game_line", title: "Game line / system", label: "game_line", name: "Game line", datatype: "text", hint: "Text, like “Worlds Without Number” or “Rifts”. Shown as a category in the browser." },
  { id: "volume", title: "Volume", label: "volume", name: "Volume", datatype: "int", hint: "A whole number, like 1 or 2." },
  // For sorting out a mixed collection.
  { id: "read_status", title: "Read status", label: "read_status", name: "Read status", datatype: "text", hint: "Text, like “to read”, “reading” or “done”." },
  { id: "source", title: "Source", label: "source", name: "Source", datatype: "text", hint: "Text, like “Downloads 2019” or “Scanned”, to remember where a book came from." },
  { id: "scan_quality", title: "Scan quality", label: "scan_quality", name: "Scan quality", datatype: "text", hint: "Text, like “good”, “poor” or “needs OCR”." },
];

/**
 * A column label made from what the person typed as its name: "Scan Quality" is `scan_quality`.
 * Lower case letters and digits, one underscore between words, no accents.
 */
export function labelFromName(name: string): string {
  return name
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

/** What the dialog explains about every custom column, in one sentence each. */
export const COLUMN_EXPLANATIONS = [
  "A custom column appears as a table column: add it with Columns…",
  "It is editable in the table and in the details panel.",
  "It appears in the category browser, so you can click a value to filter.",
  "It can be used in naming templates as {#label}.",
];

export interface ColumnDraft {
  label: string;
  name: string;
  datatype: string;
}

export function presetDraft(p: ColumnPreset): ColumnDraft {
  return { label: p.label, name: p.name, datatype: p.datatype };
}

/** Why a draft cannot be created, or null. Labels are letters, digits and underscores, lower case. */
export function columnDraftProblem(d: ColumnDraft, existing: readonly string[]): string | null {
  if (!d.label.trim()) return "A label is required.";
  if (!/^[a-z0-9_]+$/.test(d.label.trim())) return "The label can only have lower case letters, numbers and underscores.";
  if (!d.name.trim()) return "A display name is required.";
  if (existing.includes(d.label.trim())) return `There is already a column #${d.label.trim()}.`;
  return null;
}

/** Creates the column. The caller refreshes whatever shows columns. */
export async function createColumn(d: ColumnDraft, api: Pick<typeof realApi, "addCustomColumn"> = realApi): Promise<number> {
  return api.addCustomColumn(d.label.trim(), d.name.trim(), d.datatype);
}
