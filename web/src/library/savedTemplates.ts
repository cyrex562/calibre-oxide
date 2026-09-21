// A library of named, reusable templates (issue 1.15 of the #816
// epic).
//
// The template tester could evaluate a template but not keep one, so
// every useful expression had to be retyped or kept in a text file
// somewhere outside the app.
//
// Storage rides on the same named-JSON-blob mechanism reader profiles
// and toolbar prefs already use -- the fourth consumer of that
// pattern, and the reason it was worth generalising.
//
// The mutation rules live here as pure functions because the
// interesting cases are all about names colliding, and getting those
// wrong silently overwrites something the user meant to keep.

export const SAVED_TEMPLATES_PROFILE = "saved-templates";

export interface SavedTemplate {
  name: string;
  template: string;
}

export interface SavedTemplates {
  templates: SavedTemplate[];
}

export const DEFAULT_SAVED_TEMPLATES: SavedTemplates = { templates: [] };

/** Names are compared case-insensitively after trimming. */
function key(name: string): string {
  return name.trim().toLowerCase();
}

/** Thrown for input that would produce an unusable entry. */
export class TemplateNameError extends Error {}

/**
 * Adds a template, or replaces the one already under that name.
 *
 * Replacing rather than adding a duplicate is deliberate: two entries
 * with the same name are indistinguishable in a picker, and which one
 * loads would come down to array order.
 *
 * Returns a new array; the input is not mutated.
 */
export function saveTemplate(existing: SavedTemplate[], name: string, template: string): SavedTemplate[] {
  const trimmed = name.trim();
  if (!trimmed) throw new TemplateNameError("A saved template needs a name.");
  if (!template.trim()) throw new TemplateNameError("Refusing to save an empty template.");

  const k = key(trimmed);
  const out = existing.filter((t) => key(t.name) !== k);
  out.push({ name: trimmed, template });
  out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

export function removeTemplate(existing: SavedTemplate[], name: string): SavedTemplate[] {
  const k = key(name);
  return existing.filter((t) => key(t.name) !== k);
}

export function findTemplate(existing: SavedTemplate[], name: string): SavedTemplate | undefined {
  const k = key(name);
  return existing.find((t) => key(t.name) === k);
}

/**
 * Whether saving under this name would overwrite something, so the
 * UI can say so before it happens rather than after.
 */
export function wouldOverwrite(existing: SavedTemplate[], name: string): boolean {
  return findTemplate(existing, name) !== undefined;
}
