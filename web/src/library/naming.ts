// The library's naming convention: a folder template and a file template,
// stored per library on the server (`lexmechanic.naming`).
//
// It is the default wherever a template is asked for (Organize, Rename files,
// Fix titles); a run can still override it. This module holds the dialog's
// state: the two drafts, validation before anything is sent, and the live
// preview on a few sample books.

import { reactive } from "vue";
import * as realApi from "./api";
import type { NamingConvention, NamingPreview } from "./api";

export type NamingApi = Pick<typeof realApi, "fetchNaming" | "saveNaming" | "previewNaming">;

export const DEFAULT_FOLDER_TEMPLATE = "{author_sort}";
export const DEFAULT_FILE_TEMPLATE = "{title}";

/** The one template Organize takes: `folder/file` (just the file when there is no folder). */
export function combinedTemplate(folder: string, file: string): string {
  const f = folder.trim().replace(/^\/+|\/+$/g, "");
  return f ? `${f}/${file.trim()}` : file.trim();
}

/** Braces that do not pair up, which a template cannot survive. */
function braceProblem(t: string): string | null {
  let depth = 0;
  for (const ch of t.replace(/\{\{|\}\}/g, "")) {
    if (ch === "{") depth++;
    if (ch === "}") depth--;
    if (depth < 0) return "A } has no matching {.";
  }
  return depth > 0 ? "A { is never closed with }." : null;
}

/** What is wrong with the two templates before they are sent, or null. */
export function namingProblem(folder: string, file: string): string | null {
  if (!file.trim()) return "The file name template is required.";
  if (/[\\/]/.test(file)) return "The file name template cannot contain a / (put folders in the folder template).";
  return braceProblem(folder) ?? braceProblem(file);
}

export interface TokenGroup {
  title: string;
  rows: { token: string; meaning: string }[];
}

/** The short reference the dialog shows. Every form here is covered by a server test. */
export const NAMING_TOKENS: TokenGroup[] = [
  {
    title: "Fields",
    rows: [
      { token: "{title}", meaning: "the title" },
      { token: "{authors}  {author_sort}", meaning: "authors; “Last, First” for sorting" },
      { token: "{series}  {series_index}", meaning: "series and the number in it (empty when there is no series)" },
      { token: "{publisher}  {pubdate}  {languages}  {tags}", meaning: "other standard fields" },
      { token: "{#game_line}  {#edition}", meaning: "your custom columns: # and the column's label" },
    ],
  },
  {
    title: "Padding",
    rows: [
      { token: "{series_index:0>2.0f}", meaning: "7 becomes 07 (use 0>3.0f for 007)" },
    ],
  },
  {
    title: "Conditional text",
    rows: [
      { token: "{series:|| - }", meaning: "the series then “ - ”, only when there is a series" },
      { token: "{#edition: (||)}", meaning: "“ (” edition “)”, only when there is an edition" },
      { token: "{series_index:|0>2.0f| - }", meaning: "the padded number then “ - ”, only with a series" },
    ],
  },
  {
    title: "Folders",
    rows: [
      { token: "Gaming/RPG/{#game_line}", meaning: "a / starts a sub-folder; a missing value leaves no empty folder" },
    ],
  },
];

/** A worked example for the user's collection, offered as a starting point. */
export const NAMING_EXAMPLE = { folder: "Gaming/RPG/{#game_line}", file: "{series:|| }{series_index:|0>2.0f| - }{title}{#edition: (||)}" };

export function createNamingController(api: NamingApi = realApi) {
  const state = reactive({
    folder: DEFAULT_FOLDER_TEMPLATE,
    file: DEFAULT_FILE_TEMPLATE,
    saved: { folder: DEFAULT_FOLDER_TEMPLATE, file: DEFAULT_FILE_TEMPLATE },
    loaded: false,
    previews: [] as NamingPreview[],
    /** Which books the previews are for; empty means a spread over the library. */
    sampleIds: [] as number[],
    error: null as string | null,
    saving: false,
    note: "",
  });
  let previewSeq = 0;

  const problem = () => namingProblem(state.folder, state.file);
  const dirty = () => state.folder !== state.saved.folder || state.file !== state.saved.file;

  async function load(sampleIds: number[] = []): Promise<void> {
    state.sampleIds = sampleIds.slice(0, 5);
    state.note = "";
    try {
      const n: NamingConvention = await api.fetchNaming();
      state.folder = state.saved.folder = n.folder_template;
      state.file = state.saved.file = n.file_template;
      state.error = null;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
    state.loaded = true;
    await preview();
  }

  /** Runs the drafts over the sample books. A slow answer to an older edit is dropped. */
  async function preview(): Promise<void> {
    const seq = ++previewSeq;
    const bad = problem();
    if (bad) {
      state.previews = [];
      state.error = bad;
      return;
    }
    try {
      const r = await api.previewNaming(state.folder, state.file, state.sampleIds);
      if (seq !== previewSeq) return;
      state.previews = r.previews;
      state.error = null;
    } catch (e) {
      if (seq !== previewSeq) return;
      state.previews = [];
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  async function save(): Promise<boolean> {
    const bad = problem();
    if (bad) {
      state.error = bad;
      return false;
    }
    state.saving = true;
    try {
      const n = await api.saveNaming(state.folder, state.file);
      state.saved = { folder: n.folder_template, file: n.file_template };
      state.error = null;
      state.note = "Saved. Organize, Rename files and Fix titles now start from this.";
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      return false;
    } finally {
      state.saving = false;
    }
  }

  function resetToDefaults(): void {
    state.folder = DEFAULT_FOLDER_TEMPLATE;
    state.file = DEFAULT_FILE_TEMPLATE;
  }

  function useExample(): void {
    state.folder = NAMING_EXAMPLE.folder;
    state.file = NAMING_EXAMPLE.file;
  }

  return { state, problem, dirty, load, preview, save, resetToDefaults, useExample };
}

export type NamingController = ReturnType<typeof createNamingController>;
