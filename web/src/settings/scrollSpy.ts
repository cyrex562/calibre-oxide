// Which Settings section the page is "in" (C22): the nav marks it.
//
// The section whose top is the last one at or above a reading line near the top of the window.
// A page whose last section is too short to ever reach that line must not hand the highlight to
// it just because the page cannot scroll further: a click on a nav entry names its section
// directly (the caller keeps that until the reader scrolls by hand).

export interface SectionTop {
  id: string;
  /** The section's top edge, relative to the window's top (`getBoundingClientRect().top`). */
  top: number;
}

/** The id of the active section, or null when there are none. `sections` are in page order. */
export function activeSection(sections: readonly SectionTop[], line: number): string | null {
  let active: string | null = sections.length > 0 ? sections[0].id : null;
  for (const s of sections) {
    if (s.top <= line) active = s.id;
    else break;
  }
  return active;
}
