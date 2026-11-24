// The stylesheet rules that no component test renders, held by reading the CSS:
//   C23  dark skins say `color-scheme: dark`, so native controls follow them;
//   C14  a tick box or radio button in a dialog's .field row sits beside its label.

import classicRaw from "./skin-classic.css?raw";
import rawCss from "./style.css?raw";
import { describe, expect, it } from "vitest";

const css = rawCss.replace(/\r\n/g, "\n");
const classic = classicRaw.replace(/\r\n/g, "\n");

/** The declarations of the block whose selector line is `header`. */
function block(source: string, header: string): string {
  const start = source.indexOf(header);
  if (start < 0) throw new Error(`no block starts with ${header}`);
  const open = source.indexOf("{", start);
  return source.slice(open, source.indexOf("}", open));
}

describe("color-scheme", () => {
  it("is light on the base theme and dark on every dark one", () => {
    expect(block(css, ":root {")).toContain("color-scheme: light");
    expect(block(css, '  :root:not([data-theme="light"]) {')).toContain("color-scheme: dark");
    expect(block(css, ':root[data-theme="dark"] {')).toContain("color-scheme: dark");
    expect(block(classic.replace(/\r\n/g, "\n"), ':root[data-skin="classic"] {')).toContain("color-scheme: dark");
  });

  it("leaves Classic's own scrollbars in place (custom ::-webkit-scrollbar rules still apply)", () => {
    expect(classic).toContain(':root[data-skin="classic"] ::-webkit-scrollbar-thumb');
  });
});

describe("tick boxes in dialog rows", () => {
  it("lay out inline, label beside the box", () => {
    const rule = block(css, ".manage-panel .field.checkbox,");
    expect(rule).toContain("flex-direction: row");
    expect(css).toContain('.manage-panel .field:has(> input[type="checkbox"])');
    expect(css).toContain('.manage-panel .field:has(> input[type="radio"])');
  });
});
