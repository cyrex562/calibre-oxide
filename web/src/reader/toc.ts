import type { TocNode } from "./types";

export interface FlatTocEntry {
  title: string | null;
  dest: string | null;
  frag: string | null;
  depth: number;
}

/** Depth-first flatten of the manifest's TOC tree, for a simple sidebar list -- a full port of toc.pyj's bordering-node/anchor-visibility tracking is deferred, see issue #499's own doc. */
export function flattenToc(root: TocNode, depth = 0): FlatTocEntry[] {
  const out: FlatTocEntry[] = [];
  // The manifest's own root TOC node is a synthetic container (its
  // own `dest`/`frag` are typically null) -- only emit real entries.
  if (root.dest || root.title) {
    out.push({ title: root.title, dest: root.dest, frag: root.frag, depth });
  }
  for (const child of root.children ?? []) {
    out.push(...flattenToc(child, depth + 1));
  }
  return out;
}
