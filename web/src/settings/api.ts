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
