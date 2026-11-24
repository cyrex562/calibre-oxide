// The Regex checkbox sat inside `.bulk-combo`, whose `input { flex: 1 }` stretched it into a
// 48px empty bar. jsdom applies no component CSS, so this pins the selector in the source.

import rawSource from "./BulkEditPanel.vue?raw";
import { describe, expect, it } from "vitest";

const source = rawSource.replace(/\r\n/g, "\n");

describe("BulkEditPanel styles", () => {
  it("stretches the text inputs of a combo row but never a checkbox", () => {
    expect(source).toContain('.bulk-combo input:not([type="checkbox"]) {');
    expect(source).not.toMatch(/\.bulk-combo input \{\s*flex: 1/);
  });

  it("still has the checkbox the rule must leave alone", () => {
    expect(source).toMatch(/<label class="bulk-regex"><input type="checkbox"/);
  });
});
