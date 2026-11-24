// Details of the round-2 fixes that no component test renders (no layout in jsdom), held by
// reading the sources: the sticky first column, placeholders that read as examples, and the
// (the category panel's counts under a virtual library are in CategoryBrowser.vl.test.ts).

import { describe, expect, it } from "vitest";

import cssSource from "../style.css?raw";
import detailsSource from "./BookDetailsPanel.vue?raw";
import tableSource from "./BookTable.vue?raw";
import settingsSource from "../settings/SettingsView.vue?raw";

describe("the table's sticky first column (U17)", () => {
  it("marks the first column's header and cells, and pins them left over their own surface", () => {
    expect(tableSource).toContain("'sticky-col': ci === 0");
    expect(tableSource).toMatch(/\.book-table td\.sticky-col,\s*\.book-table th\.sticky-col \{\s*position: sticky;\s*left: var\(--sticky-left, 0px\);/);
    expect(tableSource).toMatch(/\.book-table td\.sticky-col \{[^}]*background: var\(--bg-field\)/);
    // The select-mode tick column leads: the first data column sits after it.
    expect(tableSource).toContain("'--sticky-left': selectMode");
  });
});

describe("placeholders (U15)", () => {
  it("are faint italic examples everywhere", () => {
    expect(cssSource).toMatch(/::placeholder \{[^}]*color: var\(--fg-faint\)[^}]*font-style: italic/);
  });

  it("start with e.g. wherever they hold an example value", () => {
    for (const source of [detailsSource, settingsSource]) {
      for (const m of source.matchAll(/placeholder="([^"]+)"/g)) {
        const text = m[1];
        // Prompts ("Save as…", "Name") describe a field; examples (an address, a list, a path) must say so.
        if (/@|example\.com|scifi|Jane Doe|\.zip|\/\/h:|^\d+$|program:/.test(text)) expect(text, `placeholder "${text}"`).toMatch(/^e\.g\. /);
      }
    }
  });
});
