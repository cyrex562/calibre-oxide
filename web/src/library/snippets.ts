// Pure parsing for /fts/snippets's highlight markers -- kept separate
// from api.ts so it's unit-testable without a network layer, matching
// query.ts's own precedent.
//
// crates/calibre_srv/src/fts.rs's own default highlight_start/_end are
// the ASCII File Separator (0x1C) / Record Separator (0x1E) control
// characters, chosen upstream specifically because real book text
// never contains them -- safe, unambiguous split points, unlike
// embedding HTML tags directly in the response (which the server
// deliberately doesn't do). Built via fromCharCode here rather than a
// literal escape, to avoid an invisible raw control byte living in
// this source file.
const HL_START = String.fromCharCode(0x1c);
const HL_END = String.fromCharCode(0x1e);

export interface SnippetSegment {
  text: string;
  highlighted: boolean;
}

// Splits one snippet's raw text (with embedded highlight markers)
// into plain/highlighted segments for rendering -- e.g. with a
// <mark> per highlighted segment -- without ever needing v-html on
// real book content.
export function parseSnippetSegments(text: string): SnippetSegment[] {
  const segments: SnippetSegment[] = [];
  let i = 0;
  while (i < text.length) {
    const start = text.indexOf(HL_START, i);
    if (start === -1) {
      segments.push({ text: text.slice(i), highlighted: false });
      break;
    }
    if (start > i) segments.push({ text: text.slice(i, start), highlighted: false });
    const end = text.indexOf(HL_END, start + 1);
    if (end === -1) {
      segments.push({ text: text.slice(start + 1), highlighted: true });
      break;
    }
    segments.push({ text: text.slice(start + 1, end), highlighted: true });
    i = end + 1;
  }
  return segments;
}
