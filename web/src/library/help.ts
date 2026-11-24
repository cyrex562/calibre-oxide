// What the Help dialog shows besides the shortcut list (C20): the core workflow as a few
// steps, each with the action that opens the dialog it talks about, and the keys that are
// not in the action registry.

import { LIBRARY_ACTIONS, type ActionGroup, type LibraryAction, type LibraryActionId } from "./actions";

export interface HelpStep {
  title: string;
  text: string;
  /** Actions that open the dialog the step is about, as buttons under it. */
  run: { id: LibraryActionId; label: string }[];
}

/** From a folder full of books to a clean, shareable library. */
export const HELP_STEPS: HelpStep[] = [
  {
    title: "Import",
    text: "Add single books, or a whole folder of them (subfolders included). Books already in the library are reported, not added twice unless you say so.",
    run: [
      { id: "add-folder", label: "Add folder…" },
      { id: "add-books", label: "Add books…" },
    ],
  },
  {
    title: "Find duplicates",
    text: "Analyze the library once, then keep one copy of each book: the others go to the Trash, where Undo brings them back. Merge instead to combine formats and metadata.",
    run: [{ id: "find-duplicates", label: "Find duplicates…" }],
  },
  {
    title: "Organize",
    text: "Move the files into folders named after your template (author, series, …). The dialog shows what will move before anything does.",
    run: [
      { id: "organize-files", label: "Organize files…" },
      { id: "file-naming", label: "File naming…" },
    ],
  },
  {
    title: "Export a copy",
    text: "Write the books you choose to another folder or drive, for travelling or as a backup. The library itself is not changed.",
    run: [{ id: "export-copy", label: "Export a copy…" }],
  },
];

export interface OtherKey {
  keys: string;
  does: string;
}

/** Keys the views handle themselves, not through the action registry. */
export const OTHER_KEYS: OtherKey[] = [
  { keys: "↑ ↓ Home End PgUp PgDn", does: "Move through the book list (Shift with them extends the selection)" },
  { keys: "Ctrl+A", does: "Select every book in the list" },
  { keys: "F2 or Enter in a cell", does: "Edit the cell in place (double-click does too)" },
  { keys: "Esc", does: "Close the open dialog or menu, or the side panel on a narrow window" },
  { keys: "PgUp PgDn ← →", does: "Turn pages in the reader" },
];

/**
 * The actions to list under one group: the visible ones, once each. Two registry entries can
 * share a label (Fix titles for everything, and for the selection), which read as a mistake.
 */
export function helpActions(group: ActionGroup, opts: { desktop: boolean; filter?: string }): LibraryAction[] {
  const needle = (opts.filter ?? "").trim().toLowerCase();
  const seen = new Set<string>();
  const out: LibraryAction[] = [];
  for (const a of LIBRARY_ACTIONS) {
    if (a.group !== group || (a.desktopOnly && !opts.desktop)) continue;
    if (seen.has(a.label)) continue;
    if (needle && !a.label.toLowerCase().includes(needle)) continue;
    seen.add(a.label);
    out.push(a);
  }
  return out;
}
