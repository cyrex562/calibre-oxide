import { describe, expect, it } from "vitest";

import { findTemplate, removeTemplate, saveTemplate, TemplateNameError, wouldOverwrite, type SavedTemplate } from "./savedTemplates";

const existing: SavedTemplate[] = [
  { name: "Author sort", template: "{author_sort}" },
  { name: "Title", template: "{title}" },
];

describe("saveTemplate", () => {
  it("adds a new template and keeps the list sorted", () => {
    const out = saveTemplate(existing, "Series", "{series}");
    expect(out.map((t) => t.name)).toEqual(["Author sort", "Series", "Title"]);
  });

  // Two entries with the same name are indistinguishable in a picker,
  // and which one loads would come down to array order.
  it("replaces an existing name rather than adding a duplicate", () => {
    const out = saveTemplate(existing, "Title", "{title} ({series})");
    expect(out).toHaveLength(2);
    expect(findTemplate(out, "Title")?.template).toBe("{title} ({series})");
  });

  it("matches names case-insensitively and after trimming", () => {
    const out = saveTemplate(existing, "  tITLE  ", "{x}");
    expect(out).toHaveLength(2);
    // The new spelling wins -- the user just typed it deliberately.
    expect(findTemplate(out, "title")?.name).toBe("tITLE");
  });

  it("does not mutate the input", () => {
    const before = [...existing];
    saveTemplate(existing, "New", "{x}");
    expect(existing).toEqual(before);
  });

  it("refuses a blank name", () => {
    expect(() => saveTemplate(existing, "   ", "{x}")).toThrow(TemplateNameError);
  });

  // Saving an empty template is always a mistake, and it would
  // silently shadow a good one if the name already existed.
  it("refuses an empty template", () => {
    expect(() => saveTemplate(existing, "Name", "   ")).toThrow(TemplateNameError);
  });
});

describe("removeTemplate", () => {
  it("removes by name, case-insensitively", () => {
    expect(removeTemplate(existing, "title").map((t) => t.name)).toEqual(["Author sort"]);
  });

  it("leaves the list alone when nothing matches", () => {
    expect(removeTemplate(existing, "Nope")).toHaveLength(2);
  });
});

describe("wouldOverwrite", () => {
  it("warns before a save silently replaces something", () => {
    expect(wouldOverwrite(existing, "Title")).toBe(true);
    expect(wouldOverwrite(existing, " title ")).toBe(true);
    expect(wouldOverwrite(existing, "Brand new")).toBe(false);
  });
});
