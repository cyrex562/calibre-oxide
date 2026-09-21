// Popup footnotes (issue 2.6 of the #816 epic).
//
// A footnote link navigated away from the page, so reading a note
// meant losing your place and navigating back. Popping the note over
// the text is what every other reader does, and what makes annotated
// editions usable.
//
// # How a note reference is recognised
//
// Verified against a real EPUB served by this project's own stack,
// not inferred: `epub:type` survives serialization **verbatim** as
// `epub:type`. `unserialize.ts`'s note about `Dom` collapsing foreign
// attribute prefixes refers to namespace *tuples* in the serialized
// form, not to the attribute name -- reading it the other way (and
// looking for a bare `type`) would have found nothing.
//
// Both the EPUB 3 `epub:type` vocabulary and the ARIA DPUB `role` are
// checked, because real books in the wild carry one, the other, or
// both.

import { isComment, type SerializedNode } from "./types";

/** `epub:type`/`role` values that mark a link as pointing at a note. */
const NOTEREF_VALUES = ["noteref", "doc-noteref"];

/** `epub:type`/`role` values that mark an element as being a note. */
const NOTE_VALUES = ["footnote", "endnote", "note", "rearnote", "doc-footnote", "doc-endnote"];

function hasAnyToken(value: string | null | undefined, wanted: string[]): boolean {
  if (!value) return false;
  // These attributes are space-separated token lists, so a substring
  // test would match "not-a-noteref" and similar.
  const tokens = value.toLowerCase().split(/\s+/).filter(Boolean);
  return tokens.some((t) => wanted.includes(t));
}

/** Whether an anchor points at a footnote rather than ordinary content. */
export function isNoteReference(epubType: string | null | undefined, role: string | null | undefined): boolean {
  return hasAnyToken(epubType, NOTEREF_VALUES) || hasAnyToken(role, NOTEREF_VALUES);
}

/** Whether an element *is* a note body. */
export function isNoteBody(epubType: string | null | undefined, role: string | null | undefined): boolean {
  return hasAnyToken(epubType, NOTE_VALUES) || hasAnyToken(role, NOTE_VALUES);
}

/** Reads an attribute off a serialized element. */
function attr(node: SerializedNode, name: string): string | null {
  if (isComment(node)) return null;
  for (const [key, value] of node.a ?? []) {
    if (key === name) return value;
  }
  return null;
}

/**
 * Finds the element carrying `id`, anywhere in the tree.
 *
 * Returns the node itself rather than its text so the caller can
 * decide what to do with it -- a note body is sometimes a whole
 * `<aside>` of paragraphs and sometimes a bare `<li>`.
 */
export function findNodeById(node: SerializedNode, id: string): SerializedNode | null {
  if (isComment(node)) return null;
  if (attr(node, "id") === id) return node;
  for (const child of node.c ?? []) {
    const found = findNodeById(child, id);
    if (found) return found;
  }
  return null;
}

/**
 * The readable text of a note, or `null` when there is no such id.
 *
 * The note's own back-link is dropped. Almost every real footnote
 * ends with a "↩" or "back" anchor pointing at the reference, which
 * is navigation furniture rather than note content and reads as
 * noise inside a popup that the reader dismisses by clicking away.
 */
export function noteTextFor(tree: SerializedNode, id: string): string | null {
  const node = findNodeById(tree, id);
  if (!node) return null;

  const parts: string[] = [];
  collect(node, parts, true);
  const text = parts.join(" ").replace(/\s+/g, " ").trim();
  return text;
}

function collect(node: SerializedNode, out: string[], isRoot: boolean): void {
  if (isComment(node)) {
    if (!isRoot && node.l) out.push(node.l);
    return;
  }

  // Skip back-links: an anchor inside a note that points back at its
  // own reference.
  if (!isRoot && node.n?.toLowerCase() === "a" && isBackLink(node)) {
    if (node.l) out.push(node.l);
    return;
  }

  if (node.x) out.push(node.x);
  for (const child of node.c ?? []) collect(child, out, false);
  // The root's tail belongs to whatever followed the note in the
  // document, not to the note.
  if (!isRoot && node.l) out.push(node.l);
}

function isBackLink(node: SerializedNode): boolean {
  const epubType = attr(node, "epub:type");
  const role = attr(node, "role");
  if (hasAnyToken(epubType, ["backlink"]) || hasAnyToken(role, ["doc-backlink"])) return true;
  // Unmarked back-links are common; they are recognisable by being a
  // bare arrow or a one-word "back".
  const text = (isComment(node) ? "" : node.x ?? "").trim();
  return text === "↩" || text === "↑" || text.toLowerCase() === "back";
}
