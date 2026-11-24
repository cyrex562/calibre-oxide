// Row colours (C16): a coloring rule tints the row and draws a bar at its edge, and the text
// keeps the theme's own colour. The tint share is --row-tint in style.css; this holds it to
// 4.5:1 for ANY colour a rule can return on every skin, by checking the eight corners of the RGB
// cube (luminance is monotonic in each channel, so the worst colour is always a corner).

import rawCss from "./style.css?raw";
import tableSource from "./components/BookTable.vue?raw";
import gridSource from "./components/BookGrid.vue?raw";
import { describe, expect, it } from "vitest";

const css = rawCss.replace(/\r\n/g, "\n");

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
const THEMES: [string, Record<string, string>][] = [
  ["Modern light", LIGHT],
  ["Modern dark (system)", { ...LIGHT, ...tokens('  :root:not([data-theme="light"]) {') }],
  ["Modern dark (chosen)", { ...LIGHT, ...tokens(':root[data-theme="dark"] {') }],
  ["Classic", { ...LIGHT, ...tokens(':root[data-skin="classic"] {\n  --font-ui') }],
];

const tint = (() => {
  const m = css.match(/--row-tint:\s*(\d+)%/);
  if (!m) throw new Error("--row-tint is not defined in style.css");
  return Number(m[1]) / 100;
})();

const rgb = (hex: string): number[] => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
function luminance(c: number[]): number {
  const [r, g, b] = c.map((v) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(a: number[], b: number[]): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
/** `color-mix(in srgb, color tint, base)`. */
const mix = (color: number[], base: number[]): number[] => color.map((v, i) => v * tint + base[i] * (1 - tint));

const CORNERS: number[][] = [];
for (const r of [0, 255]) for (const g of [0, 255]) for (const b of [0, 255]) CORNERS.push([r, g, b]);

describe("row colour tint", () => {
  it("is a real share", () => {
    expect(tint).toBeGreaterThan(0.1);
    expect(tint).toBeLessThan(0.5);
  });

  it.each(THEMES)("keeps the text readable (4.5:1) on %s for any colour, hovered rows included", (_name, t) => {
    // The field colour is what the table sits on; Modern has none of its own (it is --bg).
    const base = rgb(t["bg-field"] ?? t["bg"]);
    const fg = rgb(t["fg"]);
    for (const corner of CORNERS) {
      expect(contrast(fg, mix(corner, base)), `--fg on ${corner.join(",")} at ${tint}`).toBeGreaterThanOrEqual(4.5);
      // Hovered: the table adds 10 points of tint.
      const hover = corner.map((v, i) => v * (tint + 0.1) + base[i] * (1 - tint - 0.1));
      expect(contrast(fg, hover), `--fg hovered on ${corner.join(",")}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("is applied to the table rows and the grid cards as a tint plus a bar, not as the text colour", () => {
    for (const source of [tableSource, gridSource]) {
      expect(source).toContain("color-mix(in srgb, var(--row-color) var(--row-tint)");
      expect(source).toContain("inset 4px 0 0 var(--row-color)");
    }
    expect(tableSource).not.toMatch(/\{\s*color:\s*rowColors/);
  });
});
