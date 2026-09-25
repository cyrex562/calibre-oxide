// The Sort dropdown's contents and the effect of choosing an entry.
//
// This replaced two `<select>`s plus a row of removable chips -- the
// most form-like control in the window. The model underneath is
// unchanged: `sort` is one comma-joined string, exactly what
// `/ajax/search` wants and what library preferences already persist.
//
// Extracted from the component because one flat menu carries three
// different kinds of choice -- which field leads, which direction, and
// which extra field to append or drop -- and they are distinguished
// only by an id prefix. Getting that wrong silently sorts by the wrong
// thing rather than erroring.

export type SortMenuId = `field:${string}` | `add:${string}` | `drop:${string}` | "dir:asc" | "dir:desc";

export interface SortState {
  /** Comma-joined, primary field first. */
  sort: string;
  order: "asc" | "desc";
}

export function sortFieldsOf(sort: string): string[] {
  return sort.split(",").map((s) => s.trim()).filter(Boolean);
}

export function primaryOf(sort: string, fallback = "timestamp"): string {
  return sortFieldsOf(sort)[0] ?? fallback;
}

/**
 * Applies a menu choice, returning the new state.
 *
 * Pure, so the component only has to assign the result.
 */
export function applySortChoice(state: SortState, id: SortMenuId): SortState {
  const fields = sortFieldsOf(state.sort);

  if (id === "dir:asc") return { ...state, order: "asc" };
  if (id === "dir:desc") return { ...state, order: "desc" };

  if (id.startsWith("field:")) {
    const field = id.slice("field:".length);
    // Promoting a field that is already a secondary sort moves it to
    // the front rather than duplicating it -- otherwise "title" could
    // end up in the string twice and the server would sort by it, then
    // by it again.
    const rest = fields.filter((f) => f !== field);
    return { ...state, sort: [field, ...rest].join(",") };
  }

  if (id.startsWith("add:")) {
    const field = id.slice("add:".length);
    if (fields.includes(field)) return state;
    return { ...state, sort: [...fields, field].join(",") };
  }

  if (id.startsWith("drop:")) {
    const field = id.slice("drop:".length);
    const next = fields.filter((f) => f !== field);
    // Never leave the sort empty: dropping everything would send an
    // empty `sort` and let the server pick, which looks like the list
    // randomly reordering itself.
    return next.length > 0 ? { ...state, sort: next.join(",") } : state;
  }

  return state;
}

/** One line describing the sort, for a button title and the status bar. */
export function sortSummary(state: SortState, label: (key: string) => string): string {
  const fields = sortFieldsOf(state.sort);
  const names = (fields.length > 0 ? fields : ["timestamp"]).map(label);
  return `${names.join(", then ")} (${state.order === "asc" ? "ascending" : "descending"})`;
}
