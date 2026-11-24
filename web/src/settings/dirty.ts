// Which settings sections have edits that are not saved yet.
//
// The settings page keeps one Save for all of its sections. Each section is
// compared, as JSON, with what it held when it was last loaded or saved; this is
// the comparison and the sentence that says what is unsaved.

export type SectionKey = "library" | "reading" | "keyboard" | "toolbar" | "colors" | "email";

/** The name of each saved section, as the page's nav and the save bar say it. */
export const SECTION_LABEL: Record<SectionKey, string> = {
  library: "Library",
  reading: "Reading",
  keyboard: "Keyboard shortcuts",
  toolbar: "Toolbar",
  colors: "Row colors",
  email: "Email",
};

export const SECTION_ORDER: SectionKey[] = ["library", "reading", "keyboard", "toolbar", "colors", "email"];

/**
 * The sections whose current value differs from the baseline. A section with no
 * baseline yet (its data has not loaded) is never reported: nothing was edited.
 */
export function dirtySections(baseline: Partial<Record<SectionKey, string>>, current: Record<SectionKey, string>): SectionKey[] {
  return SECTION_ORDER.filter((k) => baseline[k] !== undefined && baseline[k] !== current[k]);
}

/** "2 unsaved changes: Library, Keyboard shortcuts" for the save bar; "" when there are none. */
export function describeUnsaved(keys: SectionKey[]): string {
  if (keys.length === 0) return "";
  return `${keys.length} unsaved ${keys.length === 1 ? "change" : "changes"}: ${keys.map((k) => SECTION_LABEL[k]).join(", ")}`;
}
