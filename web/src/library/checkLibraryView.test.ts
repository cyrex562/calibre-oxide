import { describe, expect, it } from "vitest";

import { findingName } from "./checkLibraryView";

describe("findingName", () => {
  it("is the title when there is one", () => {
    expect(findingName({ title: "The Algorithms of Theory", path: "Fiction/Fantasy/scan0030.pdf" })).toBe("The Algorithms of Theory");
  });
  it("is the file name for a file no book claims", () => {
    expect(findingName({ title: "", path: "Tech/stray notes.txt" })).toBe("stray notes.txt");
    expect(findingName({ title: "", path: "Scans\\stray-extra.pdf" })).toBe("stray-extra.pdf");
  });
  it("falls back to a dash for nothing at all", () => {
    expect(findingName({ title: "", path: "" })).toBe("—");
  });
});
