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
];

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
