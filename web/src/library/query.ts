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

/// Which shared attributes make two books "similar". Upstream offers
/// the same four as separate menu entries; combining them into one
/// action with toggles is a deliberate simplification.
export interface SimilarityBasis {
  authors?: boolean;
  tags?: boolean;
  series?: boolean;
  publisher?: boolean;
}

/** The subset of a book row `similarBooksQuery` reads. */
export interface SimilarBookSource {
  id: number;
  authors?: string[] | null;
  tags?: string[] | null;
  series?: string | null;
  publisher?: string | null;
}

/**
 * Builds a search for books sharing something with `book`, or `null`
 * when the book has nothing to match on.
 *
 * Returning `null` rather than an always-empty query matters: the
 * caller can say "this book has no authors, tags, series or publisher
 * to find similar books by" instead of silently showing zero results,
 * which looks identical to "nothing is similar".
 *
 * The book itself is excluded -- it trivially matches every clause,
 * and a "similar books" list whose first entry is the book you came
 * from is just noise.
 *
 * Syntax verified against a live server: `or` and `and not` are both
 * real (`calibre_db::search`'s And/Or/Not evaluator), and `=` forces
 * exact rather than substring matching.
 */
export function similarBooksQuery(book: SimilarBookSource, basis: SimilarityBasis): string | null {
  const clauses: string[] = [];

  if (basis.authors) {
    for (const a of book.authors ?? []) if (a) clauses.push(categoryItemToQuery("authors", a));
  }
  if (basis.tags) {
    for (const t of book.tags ?? []) if (t) clauses.push(categoryItemToQuery("tags", t));
  }
  if (basis.series && book.series) clauses.push(categoryItemToQuery("series", book.series));
  if (basis.publisher && book.publisher) clauses.push(categoryItemToQuery("publisher", book.publisher));

  if (clauses.length === 0) return null;

  const joined = clauses.length === 1 ? clauses[0] : `(${clauses.join(" or ")})`;
  return `${joined} and not id:${book.id}`;
}
