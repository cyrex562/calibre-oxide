// Finding a search hit inside the rendered chapter.
//
// The search runs over the serialized book text, which knows nothing of
// the iframe. To land on the exact hit (not just the chapter), the same
// query is run over the text the iframe actually shows, and the n-th hit
// there is the n-th hit the search listed.

import type { Match } from "./search";

export interface TextIndex {
  text: string;
  nodes: { node: Text; start: number }[];
}

const SKIPPED = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE"]);

/** The visible text of `root` with the offset each text node starts at. */
export function indexText(root: Node): TextIndex {
  const nodes: TextIndex["nodes"] = [];
  let text = "";
  const walk = (node: Node) => {
    if (node.nodeType === 3) {
      nodes.push({ node: node as Text, start: text.length });
      text += (node as Text).data;
      return;
    }
    if (node.nodeType === 1 && SKIPPED.has((node as Element).tagName.toUpperCase())) return;
    for (const child of Array.from(node.childNodes)) walk(child);
  };
  walk(root);
  return { text, nodes };
}

/** A DOM range for the characters [start, end) of an indexed text. */
export function rangeForOffsets(doc: Document, index: TextIndex, start: number, end: number): Range | null {
  const locate = (offset: number, preferEnd: boolean): { node: Text; offset: number } | null => {
    for (const { node, start: nodeStart } of index.nodes) {
      const len = node.data.length;
      if (offset < nodeStart + len || (preferEnd && offset === nodeStart + len)) return { node, offset: offset - nodeStart };
    }
    return null;
  };
  const from = locate(start, false);
  const to = locate(end, true);
  if (!from || !to) return null;
  const range = doc.createRange();
  range.setStart(from.node, from.offset);
  range.setEnd(to.node, to.offset);
  return range;
}

/**
 * Which of the chapter's own matches corresponds to the `ordinal`-th match
 * the search listed.
 *
 * When both counted the same number of matches the ordinal is exact. When
 * they disagree (the two texts break blocks differently) the surrounding
 * context decides, and if that finds nothing the answer is null -- the
 * caller then just opens the chapter, which is honest rather than wrong.
 */
export function pickMatch(domMatches: Match[], ordinal: number, listed: Match, listedCount: number): Match | null {
  if (domMatches.length === listedCount && domMatches[ordinal]) return domMatches[ordinal];
  const sameContext = domMatches.filter((m) => m.context === listed.context);
  return sameContext.length === 1 ? sameContext[0] : null;
}
