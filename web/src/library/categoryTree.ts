// State for the categories panel (the tag browser).
//
// The panel was an accordion: one category open at a time, refetching
// its items on every toggle, so opening Tags closed Authors and
// reopening Authors fetched it again. calibre's is a persistent tree
// where several categories stay open and expansion survives a restart.
//
// The parts worth testing without a browser are which categories are
// open, how that survives a reload, and how the find box filters --
// none of which need a mount.

/** Category key, e.g. `"authors"` — the last path segment of its URL. */
export type CategoryKey = string;

export const EXPANDED_KEY = "calibre-oxide.categories.expanded";

/** The key a category's URL identifies it by. */
export function keyOf(url: string): CategoryKey {
  return url.split("/").filter(Boolean).pop() ?? "";
}

/**
 * Reads persisted expansion state.
 *
 * Anything malformed collapses to "nothing expanded" rather than
 * throwing: a panel that renders closed is recoverable with one click,
 * a panel that throws takes the window with it.
 */
export function parseExpanded(raw: string | null): Set<CategoryKey> {
  if (!raw) return new Set();
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((k): k is string => typeof k === "string"));
  } catch {
    return new Set();
  }
}

export function serializeExpanded(expanded: Set<CategoryKey>): string {
  return JSON.stringify([...expanded]);
}

export function toggleExpanded(expanded: Set<CategoryKey>, key: CategoryKey): Set<CategoryKey> {
  const next = new Set(expanded);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export interface FilterableItem {
  name: string;
  count: number;
}

/**
 * Filters a category's items by the find box.
 *
 * Substring and case-insensitive by default; a leading `=` means
 * exact. Both are calibre's own semantics (`tag_browser/ui.py`), and
 * worth matching because anyone coming from calibre will type them.
 */
export function matchesFilter(name: string, filter: string): boolean {
  const trimmed = filter.trim();
  if (!trimmed) return true;
  if (trimmed.startsWith("=")) return name.toLowerCase() === trimmed.slice(1).trim().toLowerCase();
  return name.toLowerCase().includes(trimmed.toLowerCase());
}

/**
 * Splits a filter into a category scope and the rest.
 *
 * `tags:foo` searches only within Tags, matching calibre. The scope is
 * only honoured when it names a category that actually exists —
 * otherwise a search for an author called "Smith: A Life" would
 * silently scope to a nonexistent "Smith" category and find nothing.
 */
export function splitScopedFilter(filter: string, known: CategoryKey[]): { scope: CategoryKey | null; text: string } {
  const at = filter.indexOf(":");
  if (at <= 0) return { scope: null, text: filter };
  const candidate = filter.slice(0, at).trim().toLowerCase();
  if (!known.includes(candidate)) return { scope: null, text: filter };
  return { scope: candidate, text: filter.slice(at + 1) };
}

/** Which categories a filter should force open, so matches are visible. */
export function categoriesToReveal(
  filter: string,
  known: CategoryKey[],
  itemsByCategory: Record<CategoryKey, FilterableItem[]>,
): Set<CategoryKey> {
  const { scope, text } = splitScopedFilter(filter, known);
  if (!text.trim()) return new Set();
  const out = new Set<CategoryKey>();
  for (const key of scope ? [scope] : known) {
    const items = itemsByCategory[key] ?? [];
    if (items.some((item) => matchesFilter(item.name, text))) out.add(key);
  }
  return out;
}
