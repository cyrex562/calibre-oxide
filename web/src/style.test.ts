// The color tokens, held to WCAG AA (4.5:1 for text). The review found Modern's faint text
// at 2.8 to 3.9:1 and white on the dark accent at 3.4:1, which no test would have caught:
// the colors are CSS, and nothing renders them in CI. So this reads the token blocks from
// style.css and computes the ratios itself.

import rawCss from "./style.css?raw";
import { describe, expect, it } from "vitest";

const css = rawCss.replace(/\r\n/g, "\n");

/** The `--token: #hex` declarations of the first block that starts with `header`. */
function tokens(header: string): Record<string, string> {
  const start = css.indexOf(header);
  if (start < 0) throw new Error(`no block starts with ${header}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const out: Record<string, string> = {};
  for (const m of css.slice(open, close).matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})\b/g)) out[m[1]] = m[2];
  return out;
}

const LIGHT = tokens(":root {");
const DARK = { ...LIGHT, ...tokens('  :root:not([data-theme="light"]) {') };
const DARK_EXPLICIT = { ...LIGHT, ...tokens(':root[data-theme="dark"] {') };
const CLASSIC = { ...LIGHT, ...tokens(':root[data-skin="classic"] {\n  --font-ui') };

function luminance(hex: string): number {
  const channel = (i: number) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  }
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const THEMES: [string, Record<string, string>][] = [
  ["Modern light", LIGHT],
  ["Modern dark (system)", DARK],
  ["Modern dark (chosen)", DARK_EXPLICIT],
  ["Classic", CLASSIC],
];

describe("text contrast", () => {
  it("reads the token blocks it checks", () => {
    for (const [name, t] of THEMES) {
      for (const k of ["bg", "bg-sunken", "bg-bar", "fg", "fg-muted", "fg-faint", "fg-on-accent", "accent-solid"]) expect(t[k], `${name} has no --${k}`).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });

  // Counts, twisties and the "All books" label are --fg-faint on the panels they sit on.
  it.each(THEMES)("--fg-faint and --fg-muted are readable on every surface in %s", (_name, t) => {
    for (const surface of ["bg", "bg-sunken", "bg-bar", "bg-row-alt"]) {
      expect(contrast(t["fg-faint"], t[surface]), `--fg-faint on --${surface}`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t["fg-muted"], t[surface]), `--fg-muted on --${surface}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  // Primary buttons, the active Table/Grid toggle and a highlighted suggestion all put
  // --fg-on-accent on this fill; it is what white-on-#4d86ff (3.4:1) was not.
  it.each(THEMES)("white text on the accent fill, and on its hover, is readable in %s", (_name, t) => {
    expect(contrast(t["fg-on-accent"], t["accent-solid"])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(t["fg-on-accent"], t["accent-solid-hover"])).toBeGreaterThanOrEqual(4.5);
  });

  it("the primary button uses the accent fill, not the lighter link color", () => {
    expect(css).toMatch(/button\.primary \{\s*background: var\(--accent-solid\);/);
  });
});
