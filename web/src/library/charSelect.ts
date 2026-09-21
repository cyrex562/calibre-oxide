// Character picker for the editor (the last part of issue 3.6 in the
// #816 epic).
//
// Typography is most of what hand-editing an EPUB is for, and the
// characters that matter — real quotes, real dashes, non-breaking
// spaces — are precisely the ones with no key on the keyboard. Typing
// them means a web search or a memorised alt-code.
//
// # Why the list is curated rather than the whole of Unicode
//
// Upstream ships a full character-map dialog with name search over
// every codepoint. That is a different, larger feature; a book editor
// reaches for the same two dozen characters almost every time. A
// short, named, searchable list is more useful more often than a
// grid of 150,000 glyphs, and honest about being a subset.
//
// Invisible characters are the reason each entry carries a name: a
// non-breaking space and a normal one are indistinguishable in a
// grid, and inserting the wrong one produces a bug nobody can see.

export interface SpecialChar {
  char: string;
  name: string;
  /** Extra words to match when searching. */
  keywords?: string;
}

export const CHAR_GROUPS: { group: string; chars: SpecialChar[] }[] = [
  {
    group: "Quotes",
    chars: [
      { char: "“", name: "Left double quote", keywords: "curly smart open" },
      { char: "”", name: "Right double quote", keywords: "curly smart close" },
      { char: "‘", name: "Left single quote", keywords: "curly smart open" },
      { char: "’", name: "Right single quote", keywords: "curly smart close apostrophe" },
      { char: "«", name: "Left guillemet", keywords: "french angle" },
      { char: "»", name: "Right guillemet", keywords: "french angle" },
      { char: "„", name: "Low double quote", keywords: "german" },
    ],
  },
  {
    group: "Dashes and spaces",
    chars: [
      { char: "—", name: "Em dash", keywords: "long" },
      { char: "–", name: "En dash", keywords: "range" },
      { char: "‐", name: "Hyphen", keywords: "true real" },
      { char: "­", name: "Soft hyphen", keywords: "shy break invisible" },
      { char: " ", name: "Non-breaking space", keywords: "nbsp invisible" },
      { char: " ", name: "Thin space", keywords: "invisible" },
      { char: "​", name: "Zero-width space", keywords: "invisible break" },
    ],
  },
  {
    group: "Punctuation",
    chars: [
      { char: "…", name: "Ellipsis", keywords: "dots" },
      { char: "·", name: "Middle dot", keywords: "interpunct" },
      { char: "•", name: "Bullet", keywords: "list" },
      { char: "†", name: "Dagger", keywords: "footnote" },
      { char: "‡", name: "Double dagger", keywords: "footnote" },
      { char: "§", name: "Section sign", keywords: "legal" },
      { char: "¶", name: "Pilcrow", keywords: "paragraph" },
      { char: "′", name: "Prime", keywords: "minutes feet" },
      { char: "″", name: "Double prime", keywords: "seconds inches" },
    ],
  },
  {
    group: "Symbols",
    chars: [
      { char: "©", name: "Copyright" },
      { char: "®", name: "Registered" },
      { char: "™", name: "Trademark" },
      { char: "°", name: "Degree" },
      { char: "×", name: "Multiplication", keywords: "times" },
      { char: "−", name: "Minus", keywords: "true real" },
      { char: "½", name: "One half", keywords: "fraction" },
      { char: "¼", name: "One quarter", keywords: "fraction" },
      { char: "¾", name: "Three quarters", keywords: "fraction" },
      { char: "→", name: "Right arrow" },
      { char: "←", name: "Left arrow" },
      { char: "↩", name: "Return arrow", keywords: "footnote back" },
    ],
  },
];

/** Every character, flattened. */
export function allChars(): SpecialChar[] {
  return CHAR_GROUPS.flatMap((g) => g.chars);
}

/**
 * Characters matching a search.
 *
 * Matches the name, the keywords, and the character itself — pasting
 * a character you already have in order to find its name is a real
 * way to use this.
 */
export function searchChars(query: string): SpecialChar[] {
  const q = query.trim().toLowerCase();
  if (!q) return allChars();
  return allChars().filter((c) => c.name.toLowerCase().includes(q) || (c.keywords ?? "").includes(q) || c.char === query.trim());
}

/**
 * Inserts `char` into `text` at `cursor`, returning the new text and
 * where the cursor should end up.
 *
 * A selection is replaced rather than appended to, matching every
 * editor. The returned position is after the inserted character, so
 * typing can continue — leaving it where it was would put the next
 * keystroke on the wrong side.
 */
export function insertAt(text: string, selectionStart: number, selectionEnd: number, char: string): { text: string; cursor: number } {
  const start = Math.max(0, Math.min(selectionStart, text.length));
  const end = Math.max(start, Math.min(selectionEnd, text.length));
  return { text: text.slice(0, start) + char + text.slice(end), cursor: start + char.length };
}
