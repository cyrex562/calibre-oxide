// Row colouring rules (issue 4.1 of the #816 epic).
//
// A rule is a template evaluated against each book; whatever it
// returns is the colour for that row. That is upstream's own model,
// and it is why colouring needed the bulk template-evaluation route
// -- a page of results is one evaluation per row.
//
// # Why the matching is pure
//
// The interesting cases are all about precedence and trust: which
// rule wins when several match, and what happens when a template
// returns something that is not a colour at all. A template can
// return any string, and that string ends up in a `style` attribute
// -- so it has to be checked, not interpolated hopefully.

export interface ColoringRule {
  /** Shown in the settings list. */
  name: string;
  /** A calibre template. A non-empty result colours the row. */
  template: string;
  enabled: boolean;
}

export const COLORING_RULES_PROFILE = "coloring-rules";

export interface ColoringRulesPrefs {
  rules: ColoringRule[];
}

export const DEFAULT_COLORING_RULES: ColoringRulesPrefs = { rules: [] };

/**
 * Colours a template may return.
 *
 * An allowlist rather than a syntax check: the value goes into a
 * `style` attribute, and a template is user-authored text that could
 * return anything -- `red; background: url(...)` among them. Naming
 * the acceptable values means nothing else can get through, which a
 * regex over CSS colour syntax could not promise.
 */
export const ALLOWED_COLORS = [
  "red",
  "orange",
  "yellow",
  "green",
  "blue",
  "purple",
  "brown",
  "gray",
  "grey",
  "black",
  "white",
  "pink",
  "cyan",
  "magenta",
  "teal",
  "olive",
  "navy",
  "maroon",
] as const;

const ALLOWED = new Set<string>(ALLOWED_COLORS);

/**
 * The colour a template result denotes, or `null`.
 *
 * `null` covers both "this rule did not match" (an empty result) and
 * "this rule returned something that is not a colour" -- the caller
 * treats them the same, because a rule whose output cannot be shown
 * should fall through to the next rule rather than colouring a row
 * wrongly or not at all.
 */
export function colorFromResult(result: string | undefined | null): string | null {
  if (!result) return null;
  const value = result.trim().toLowerCase();
  if (!value) return null;
  if (ALLOWED.has(value)) return value;
  // `#rgb` / `#rrggbb` are unambiguous and cannot carry a payload.
  if (/^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/.test(value)) return value;
  return null;
}

/**
 * The colour for one book, given each rule's evaluated result.
 *
 * First enabled rule that yields a usable colour wins, so the list
 * order in settings *is* the precedence -- which is what a user
 * reordering that list expects.
 */
export function colorForBook(rules: ColoringRule[], resultsByRule: (string | undefined)[]): string | null {
  for (let i = 0; i < rules.length; i += 1) {
    if (!rules[i].enabled) continue;
    const color = colorFromResult(resultsByRule[i]);
    if (color) return color;
  }
  return null;
}

/** Rules worth sending to the server: enabled, and actually written. */
export function activeRules(rules: ColoringRule[]): ColoringRule[] {
  return rules.filter((r) => r.enabled && r.template.trim() !== "");
}
