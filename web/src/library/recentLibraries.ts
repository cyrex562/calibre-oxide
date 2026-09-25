// The recent-libraries block in the library menu.
//
// Extracted because two rules here are invisible when wrong: which
// entry is omitted, and how a path becomes a label. Both silently
// produce a plausible menu rather than an error.

export interface RecentLibraryEntry {
  /** `lib:<path>` — the path travels in the id. */
  id: `lib:${string}`;
  label: string;
  path: string;
}

/** The folder name, which is what a user calls a library. */
export function shortLibraryName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/**
 * Recents, minus the one already open.
 *
 * Switching to where you already are would restart the backend and
 * reload the window for no change, so the current library is listed
 * nowhere — the toolbar button already wears its name.
 */
export function recentLibraryEntries(paths: string[], current: string): RecentLibraryEntry[] {
  const seen = new Set<string>();
  const out: RecentLibraryEntry[] = [];
  for (const path of paths) {
    if (path === current) continue;
    // A path can appear twice if it was opened by two different
    // routes; the menu should not show it twice.
    if (seen.has(path)) continue;
    seen.add(path);
    out.push({ id: `lib:${path}`, label: shortLibraryName(path), path });
  }
  return out;
}

/** The path back out of an id, or `null` if it is not a library id. */
export function pathFromLibraryId(id: string): string | null {
  return id.startsWith("lib:") ? id.slice("lib:".length) : null;
}
