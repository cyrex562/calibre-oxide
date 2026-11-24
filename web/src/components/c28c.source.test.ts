// Details of the round-2 fixes that no component test renders (no layout in jsdom), held by
// reading the sources: the sticky first column, placeholders that read as examples, and the
// category panel's note about counts under a virtual library.

import { afterEach, describe, expect, it } from "vitest";
import { createApp, nextTick, type App } from "vue";

import cssSource from "../style.css?raw";
import detailsSource from "./BookDetailsPanel.vue?raw";
import tableSource from "./BookTable.vue?raw";
import settingsSource from "../settings/SettingsView.vue?raw";
import CategoryBrowser from "./CategoryBrowser.vue";

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

describe("the category panel under a virtual library (C19)", () => {
  let app: App | null = null;
  afterEach(() => {
    app?.unmount();
    app = null;
    document.body.replaceChildren();
  });

  async function mount(virtualLibrary: string) {
    const host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp(CategoryBrowser, { virtualLibrary });
    app.mount(host);
    await nextTick();
    return host;
  }

  it("says the counts are for the whole library", async () => {
    const el = await mount("Travel shelf");
    expect(el.querySelector('[data-testid="vl-count-note"]')!.textContent).toContain("whole library");
    expect(el.querySelector('[data-testid="vl-count-note"]')!.textContent).toContain("Travel shelf");
  });

  it("says nothing without one", async () => {
    const el = await mount("");
    expect(el.querySelector('[data-testid="vl-count-note"]')).toBeNull();
  });
});
