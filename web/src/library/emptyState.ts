// What the list says when it has no rows. Four different situations used to
// share one line, "No books found.": the library has no books at all (the
// user should add some), a search or filter hides every book (the user should
// loosen it), the list failed to load (retry), and it is still loading.

export type EmptyKind = "loading" | "error" | "empty-library" | "no-results" | "none";

export interface ActiveFilter {
  /** What clears it. */
  id: "search" | "virtual-library" | "folder" | "marked";
  /** "Search: dune", "Virtual library: Unread". */
  label: string;
}

export interface EmptyInputs {
  loading: boolean;
  error: string | null;
  /** Rows in the current result. */
  total: number;
  /** The committed search text, as sent. */
  query: string;
  /** The virtual library's name; "" for none. */
  virtualLibrary: string;
  showMarkedOnly: boolean;
}

const FOLDER_QUERY = /^folder:"((?:[^"\\]|\\.)*)"$/;

/** The folder a query selects (`folder:"A/B"`), or null when it is anything else. */
export function folderOfQuery(query: string): string | null {
  const m = FOLDER_QUERY.exec(query.trim());
  return m ? m[1].replace(/\\(.)/g, "$1") : null;
}

/** The filters that can be hiding books, each with the one thing that clears it. */
export function activeFilters(o: Pick<EmptyInputs, "query" | "virtualLibrary" | "showMarkedOnly">): ActiveFilter[] {
  const out: ActiveFilter[] = [];
  const q = o.query.trim();
  if (q) {
    const folder = folderOfQuery(q);
    out.push(folder !== null ? { id: "folder", label: `Folder: ${folder}` } : { id: "search", label: `Search: ${q}` });
  }
  if (o.virtualLibrary) out.push({ id: "virtual-library", label: `Virtual library: ${o.virtualLibrary}` });
  if (o.showMarkedOnly) out.push({ id: "marked", label: "Marked books only" });
  return out;
}

export function classifyEmpty(o: EmptyInputs): EmptyKind {
  if (o.error) return "error";
  if (o.loading) return o.total === 0 ? "loading" : "none";
  if (o.total > 0) return "none";
  return activeFilters(o).length > 0 ? "no-results" : "empty-library";
}
