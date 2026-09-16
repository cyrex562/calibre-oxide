// Minimal, real range-anchoring scheme for text highlights (issue
// #731) -- NOT a real EPUB CFI. This reader has no real CFI
// implementation (see position.ts's own doc for the identical
// point-scheme narrowing); a real one is calibre's own ~1000-line
// calibre.ebooks.epub.cfi, out of scope here. Scoped further per
// #731's own "option 2": a highlight only ever spans within the
// single spine file it was created in -- no cross-page ranges (real
// CFI can span the whole book; this can't).
//
// Each boundary (start or end of a highlight) is anchored as the
// spine index it belongs to, plus a dot-separated path of child-node
// indices from that spine file's own <body> down to the boundary's
// text node, plus a character offset within it -- a standard, simple
// range-anchoring technique. Stored in the same opaque start_cfi/
// end_cfi string fields real upstream's own CFI would use --
// calibre_db::annotations never parses these fields itself, only
// stores/returns them verbatim (matching position.ts's identical
// precedent for the `cfi` field), so an honestly-different format is
// the disclosed-narrowing choice here too, not a silent mismatch.
//
// This only round-trips correctly if the spine file's DOM is rebuilt
// identically on every load (the same HTML parsed the same way) --
// true here since a spine file's content never changes between loads.

const PREFIX = "calibre-oxide-simple-range:";

function pathTo(root: Node, node: Node): string | null {
  const indices: number[] = [];
  let cur: Node | null = node;
  while (cur && cur !== root) {
    const parent: Node | null = cur.parentNode;
    if (!parent) return null;
    const idx = Array.prototype.indexOf.call(parent.childNodes, cur);
    if (idx === -1) return null;
    indices.unshift(idx);
    cur = parent;
  }
  if (cur !== root) return null; // node isn't a descendant of root
  return indices.join(".");
}

function resolvePath(root: Node, path: string): Node | null {
  if (path === "") return root;
  let cur: Node = root;
  for (const part of path.split(".")) {
    const idx = Number.parseInt(part, 10);
    if (Number.isNaN(idx)) return null;
    const next: Node | undefined = cur.childNodes[idx];
    if (!next) return null;
    cur = next;
  }
  return cur;
}

export function encodeBoundary(root: Node, spineIndex: number, node: Node, offset: number): string | null {
  const path = pathTo(root, node);
  if (path === null) return null;
  return `${PREFIX}${spineIndex}:${path}:${offset}`;
}

export interface DecodedBoundary {
  spineIndex: number;
  path: string;
  offset: number;
}

export function decodeBoundary(encoded: string): DecodedBoundary | null {
  if (!encoded.startsWith(PREFIX)) return null;
  const rest = encoded.slice(PREFIX.length);
  const parts = rest.split(":");
  if (parts.length !== 3) return null;
  const spineIndex = Number.parseInt(parts[0], 10);
  const offset = Number.parseInt(parts[2], 10);
  if (Number.isNaN(spineIndex) || Number.isNaN(offset)) return null;
  return { spineIndex, path: parts[1], offset };
}

/// Resolves a previously-encoded boundary back to a real DOM position
/// against `root` (the current spine file's own document.body) --
/// `null` if the path no longer resolves (a real, honest failure mode
/// rather than throwing, since a decode is always attempted
/// speculatively against whatever spine file happens to be loaded).
export function resolveBoundary(root: Node, decoded: DecodedBoundary): { node: Node; offset: number } | null {
  const node = resolvePath(root, decoded.path);
  if (!node) return null;
  return { node, offset: decoded.offset };
}

/// Builds a real `Range` from two previously-encoded boundaries,
/// against `doc` (the current spine file's own document) -- `null` if
/// either boundary doesn't belong to `spineIndex` or fails to resolve.
export function rangeFromEncoded(doc: Document, spineIndex: number, startCfi: string, endCfi: string): Range | null {
  const start = decodeBoundary(startCfi);
  const end = decodeBoundary(endCfi);
  if (!start || !end || start.spineIndex !== spineIndex || end.spineIndex !== spineIndex) return null;
  const root = doc.body;
  const resolvedStart = resolveBoundary(root, start);
  const resolvedEnd = resolveBoundary(root, end);
  if (!resolvedStart || !resolvedEnd) return null;
  const range = doc.createRange();
  try {
    range.setStart(resolvedStart.node, resolvedStart.offset);
    range.setEnd(resolvedEnd.node, resolvedEnd.offset);
  } catch {
    return null;
  }
  return range;
}
