// In-book search (issue 2.2 of the #816 epic).
//
// The most conspicuous remaining reader gap: a book could be read but
// not searched, which for reference material is most of why you'd
// open it.
//
// # Searching a serialized tree, not HTML
//
// Spine files arrive as `reader_json` -- a serialized DOM tree, not
// raw markup -- so text extraction walks that tree rather than
// stripping tags with a regex. That is the more reliable direction
// anyway: a tag-stripping regex has to guess about entities, script
// and style content, and attribute values that merely look like text.
//
// # Everything here is pure
//
// Extraction and matching are the parts that are subtly wrong in ways
// no one notices until a search quietly misses a hit: a match
// spanning two text nodes, a query that happens to contain regex
// metacharacters, word boundaries next to punctuation. They are
// testable without a reader, an iframe or a network.

import { isComment, type SerializedNode } from "./types";

/** Elements whose text is markup, not prose. */
const NON_PROSE = new Set(["script", "style", "head", "title", "meta", "link"]);

/**
 * All readable text in a spine file, in document order.
 *
 * Block-level elements contribute a space so words either side of a
 * boundary do not run together -- without this, "end.</p><p>Next"
 * extracts as "end.Next" and a phrase search across the boundary
 * silently fails.
 *
 * The result is then whitespace-normalized. Both a block's opening
 * and closing contribute a space, and the source markup has its own
 * newlines and indentation, so without this a phrase spanning a
 * paragraph break would have to be searched for with exactly the
 * right run of spaces -- which no one could guess. Normalizing here
 * rather than at each match site also keeps every offset meaningful
 * against one canonical string.
 */
export function extractText(node: SerializedNode): string {
  const parts: string[] = [];
  walk(node, parts);
  return parts.join("").replace(/\s+/g, " ").trim();
}

const INLINE = new Set(["a", "b", "i", "em", "strong", "span", "code", "sub", "sup", "small", "u", "s", "abbr", "cite", "q", "tt", "var", "kbd", "samp", "mark", "ruby", "rt", "bdi", "bdo", "wbr", "br"]);

function walk(node: SerializedNode, out: string[]): void {
  if (isComment(node)) {
    // A comment's own text is not prose, but its tail is: it is the
    // text that followed the comment in the document.
    if (node.l) out.push(node.l);
    return;
  }

  const tag = (node.n || "").toLowerCase();
  if (NON_PROSE.has(tag)) {
    if (node.l) out.push(node.l);
    return;
  }

  const block = !INLINE.has(tag);
  if (block) out.push(" ");

  if (node.x) out.push(node.x);
  for (const child of node.c ?? []) walk(child, out);
  if (block) out.push(" ");
  if (node.l) out.push(node.l);
}

export interface SearchOptions {
  caseSensitive?: boolean;
  wholeWord?: boolean;
}

export interface Match {
  /** Character offset of the match within the extracted text. */
  index: number;
  /** The matched text, exactly as it appears. */
  text: string;
  /** Surrounding text for display. */
  context: string;
  /** Offset of the match within `context`, for highlighting it. */
  contextOffset: number;
}

/** Characters that would otherwise be read as a pattern. */
function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

const CONTEXT_RADIUS = 48;

/**
 * Every match of `query` in `text`.
 *
 * The query is escaped, not compiled: someone searching for "C++" or
 * "(1998)" means those characters literally, and treating the box as
 * a regex would either error or match something wildly unrelated.
 *
 * `wholeWord` uses explicit boundary checks rather than `\b`, which
 * is defined in terms of ASCII word characters and so misfires on
 * accented letters -- `\bécrit\b` does not match "écrit" the way a
 * reader would expect.
 */
export function findMatches(text: string, query: string, options: SearchOptions = {}): Match[] {
  const trimmed = query.trim();
  if (!trimmed) return [];

  const flags = options.caseSensitive ? "g" : "gi";
  const re = new RegExp(escapeRegex(trimmed), flags);

  const matches: Match[] = [];
  for (const m of text.matchAll(re)) {
    const index = m.index ?? 0;
    if (options.wholeWord && !isWholeWord(text, index, m[0].length)) continue;

    const start = Math.max(0, index - CONTEXT_RADIUS);
    const end = Math.min(text.length, index + m[0].length + CONTEXT_RADIUS);
    const context = text.slice(start, end).replace(/\s+/g, " ").trim();
    // Recomputed against the collapsed context rather than carried
    // over: collapsing runs of whitespace shifts every offset after
    // the first run.
    const contextOffset = context.indexOf(m[0]);

    matches.push({ index, text: m[0], context, contextOffset: contextOffset < 0 ? 0 : contextOffset });
  }
  return matches;
}

/** Whether a word character sits immediately either side of a match. */
function isWholeWord(text: string, index: number, length: number): boolean {
  const before = index > 0 ? text[index - 1] : "";
  const after = index + length < text.length ? text[index + length] : "";
  return !isWordChar(before) && !isWordChar(after);
}

function isWordChar(ch: string): boolean {
  if (!ch) return false;
  // Letters, marks and digits from any script -- not just ASCII.
  return /[\p{L}\p{M}\p{N}_]/u.test(ch);
}

export interface SpineMatches {
  /** Index into the manifest's spine array. */
  spineIndex: number;
  /** Spine file name, for display and navigation. */
  name: string;
  matches: Match[];
}

/** Total matches across every searched spine file. */
export function totalMatches(results: SpineMatches[]): number {
  return results.reduce((n, r) => n + r.matches.length, 0);
}
