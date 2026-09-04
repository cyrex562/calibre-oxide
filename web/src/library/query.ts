// Pure helpers for building calibre_db::search query strings from UI
// interactions -- kept separate from api.ts so they're unit-testable
// without a network layer.

/// Builds a `field:"=value"` exact-match search-query clause for one
/// category item (e.g. clicking "Tolkien" under Authors). `=` forces
/// an exact match (see calibre_db::search's own `strip_prefix('=')`
/// handling) rather than a substring match, matching what a user
/// expects when clicking one specific browsed item. Internal `"` is
/// escaped so a name containing a quote can't break out of the
/// quoted clause.
export function categoryItemToQuery(category: string, itemName: string): string {
  const escaped = itemName.replace(/"/g, '\\"');
  return `${category}:"=${escaped}"`;
}
