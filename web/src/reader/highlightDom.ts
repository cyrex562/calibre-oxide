// Wraps a real DOM Range in `<mark>` elements for rendering a
// persisted highlight back into the page. Not a plain
// `range.surroundContents(mark)`: that throws whenever the range's
// boundaries don't nest cleanly inside a single element (e.g. a
// highlight spanning two <p> tags) -- exactly the case a real text
// selection produces whenever it isn't confined to one text node.
// Instead, every real Text node the range touches is found and
// wrapped individually, which works regardless of how many elements
// the selection crosses.

function collectTextNodesInRange(range: Range): Text[] {
  const doc = range.startContainer.ownerDocument;
  if (!doc) return [];
  const root = range.commonAncestorContainer.nodeType === Node.TEXT_NODE ? (range.commonAncestorContainer.parentNode ?? range.commonAncestorContainer) : range.commonAncestorContainer;
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const result: Text[] = [];
  let active = false;
  let node = walker.nextNode();
  while (node) {
    if (node === range.startContainer) active = true;
    if (active) result.push(node as Text);
    if (node === range.endContainer) break;
    node = walker.nextNode();
  }
  return result;
}

/// Wraps every text node `range` touches in its own `<mark
/// class="{className}">` element (real content, not a visual overlay
/// -- reflows and selects like any other inline markup). `dataset` is
/// applied to every `<mark>` produced (e.g. the highlight's own uuid,
/// so a later "remove highlight" action can find every fragment of a
/// multi-node highlight, even though this slice doesn't implement
/// that action yet).
export function wrapHighlightRange(range: Range, className: string, dataset: Record<string, string> = {}): void {
  const nodes = collectTextNodesInRange(range);
  for (const textNode of nodes) {
    const doc = textNode.ownerDocument;
    if (!doc) continue;
    const from = textNode === range.startContainer ? range.startOffset : 0;
    const to = textNode === range.endContainer ? range.endOffset : textNode.data.length;
    if (from >= to) continue;
    const subRange = doc.createRange();
    subRange.setStart(textNode, from);
    subRange.setEnd(textNode, to);
    const mark = doc.createElement("mark");
    mark.className = className;
    for (const [key, value] of Object.entries(dataset)) mark.dataset[key] = value;
    // Safe here (unlike the caller's own full range): `subRange`
    // spans only within this single text node, which always nests
    // cleanly inside one new element.
    subRange.surroundContents(mark);
  }
}
