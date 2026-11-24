import { describe, expect, it } from "vitest";

import { describeUnsaved, dirtySections, SECTION_ORDER, type SectionKey } from "./dirty";

const current = (over: Partial<Record<SectionKey, string>> = {}): Record<SectionKey, string> => ({ library: "a", reading: "b", keyboard: "c", toolbar: "d", colors: "e", email: "f", ...over });

describe("dirtySections", () => {
  it("reports exactly the sections that differ from the baseline, in page order", () => {
    const baseline = current();
    expect(dirtySections(baseline, current())).toEqual([]);
    expect(dirtySections(baseline, current({ email: "x", library: "y" }))).toEqual(["library", "email"]);
  });

  it("does not report a section whose data has not loaded yet", () => {
    expect(dirtySections({ library: "a" }, current({ reading: "changed" }))).toEqual([]);
    expect(dirtySections({}, current())).toEqual([]);
  });

  it("covers every section", () => {
    expect(SECTION_ORDER).toHaveLength(6);
  });
});

describe("describeUnsaved", () => {
  it("names the sections with edits", () => {
    expect(describeUnsaved([])).toBe("");
    expect(describeUnsaved(["library"])).toBe("1 unsaved change: Library");
    expect(describeUnsaved(["keyboard", "email"])).toBe("2 unsaved changes: Keyboard shortcuts, Email");
  });
});
