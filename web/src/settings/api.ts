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

// Toolbar customization (#753), now sourced from the full library
// action registry (#817) rather than a list of its own.
//
// #753 built the first action registry here, deliberately scoped to
// LibraryView.vue's self-contained header buttons. That scope stopped
// being enough once context menus, keyboard shortcuts and the desktop
// native menu all needed the same catalogue, so the registry moved to
// library/actions.ts and grew per-action group/requirement metadata.
// The toolbar-eligible subset re-exported here keeps the exact shape
// this panel already consumed, and every id string is unchanged, so
// `ToolbarPrefs` blobs already persisted server-side still apply.
export { TOOLBAR_ACTIONS, type ToolbarActionId } from "../library/actions";
import type { ToolbarActionId } from "../library/actions";

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
