// Generic named-JSON-blob storage for web-only settings, built on
// calibre_srv's already-real `/reader-profiles/*` routes
// (crates/calibre_srv/src/reader_profiles.rs). That module's own doc
// already frames it as "named, per-user JSON blobs of in-browser
// settings (font size, theme, etc.)" -- a real, existing fit for
// library-view and reading preferences, not just the reader's own
// state its route name suggests. See issue #721.

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(url, init);
  if (!resp.ok) {
    throw new Error(`${init?.method ?? "GET"} ${url} failed: ${resp.status} ${resp.statusText}`);
  }
  return (await resp.json()) as T;
}

export async function fetchProfile<T>(name: string): Promise<T | null> {
  const all = await jsonFetch<Record<string, T | null>>("/reader-profiles/get-all");
  return all[name] ?? null;
}

export async function saveProfile(name: string, profile: Record<string, unknown>): Promise<void> {
  await jsonFetch<boolean>("/reader-profiles/save", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, profile }),
  });
}

export interface LibraryPrefs {
  sort: string;
  sortOrder: "asc" | "desc";
  pageSize: number;
  /// What happens when adding a book detects a same-title/author
  /// duplicate: "ask" prompts every time (the pre-#721 default
  /// behavior), "add"/"skip" act immediately with no prompt.
  duplicateDefault: "ask" | "add" | "skip";
}

export const DEFAULT_LIBRARY_PREFS: LibraryPrefs = { sort: "timestamp", sortOrder: "desc", pageSize: 24, duplicateDefault: "ask" };

export interface ReaderPrefs {
  fontSizePercent: number;
  theme: "light" | "dark" | "sepia";
}

export const DEFAULT_READER_PREFS: ReaderPrefs = { fontSizePercent: 100, theme: "light" };

export const LIBRARY_PREFS_PROFILE = "library-prefs";
export const READER_PREFS_PROFILE = "reader-prefs";

// Keyboard shortcut customization (#752). A real, deliberate first
// slice: this port's own real keyboard-triggerable action surface
// today is exactly `ReaderView.vue`'s prev/next handlers (confirmed
// via grep before assuming a larger surface existed, matching this
// issue's own filed instruction) -- nothing in `LibraryView.vue` is
// keyboard-triggerable yet. Each action gets exactly one rebindable
// key (a `KeyboardEvent.key` value, e.g. "ArrowRight"); this replaces
// the previous hardcoded PageDown/ArrowRight and PageUp/ArrowLeft
// dual-binding with a single real configurable key per action rather
// than preserving both as permanent unconfigurable fallbacks -- a
// real, disclosed narrowing, not an oversight.
export type KeymapAction = "readerNext" | "readerPrev";

export type KeymapPrefs = Record<KeymapAction, string>;

export const DEFAULT_KEYMAP: KeymapPrefs = { readerNext: "ArrowRight", readerPrev: "ArrowLeft" };

export const KEYMAP_ACTION_LABELS: Record<KeymapAction, string> = { readerNext: "Reader: next page", readerPrev: "Reader: previous page" };

export const KEYMAP_PROFILE = "keymap";

// Toolbar customization (#753). Real prerequisite this issue's own
// body called out: this port had no action-registry concept at all
// before this -- "what actions exist" was implicit in each
// component's own template. This registry is a real, deliberate first
// slice: the LibraryView.vue header's self-contained, always-simple
// action buttons (open a panel, trigger a one-shot fetch/export) --
// not the context-dependent controls (sort fields, bulk-edit, select
// mode) that only make sense with live state alongside them, and not
// ReaderView.vue's own toolbar (a real, separable follow-up if this
// slice proves out).
export type ToolbarActionId = "manage-lists" | "custom-columns" | "check-library" | "find-duplicates" | "export-catalog" | "export-library-archive" | "fetch-news" | "add-books" | "add-folder" | "switch-library";

export const TOOLBAR_ACTIONS: { id: ToolbarActionId; label: string }[] = [
  { id: "manage-lists", label: "Manage lists…" },
  { id: "custom-columns", label: "Custom columns…" },
  { id: "check-library", label: "Check library…" },
  { id: "find-duplicates", label: "Find duplicates…" },
  { id: "export-catalog", label: "Export catalog…" },
  { id: "export-library-archive", label: "Export library archive…" },
  { id: "fetch-news", label: "Fetch news…" },
  { id: "add-books", label: "Add Books…" },
  { id: "add-folder", label: "Add Folder… (desktop app only)" },
  { id: "switch-library", label: "Switch library… (desktop app only)" },
];

export interface ToolbarPrefs {
  /// Action ids hidden from the toolbar.
  hidden: ToolbarActionId[];
  /// The full left-to-right display order (every `TOOLBAR_ACTIONS` id,
  /// reordered), applied via each button's own CSS flex `order`
  /// (index in this array). Empty means "keep each action's own
  /// default template position."
  order: ToolbarActionId[];
}

export const DEFAULT_TOOLBAR_PREFS: ToolbarPrefs = { hidden: [], order: [] };

export const TOOLBAR_PREFS_PROFILE = "toolbar-prefs";
