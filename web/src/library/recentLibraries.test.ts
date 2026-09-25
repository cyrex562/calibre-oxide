import { describe, expect, it } from "vitest";

import { pathFromLibraryId, recentLibraryEntries, shortLibraryName } from "./recentLibraries";

describe("shortLibraryName", () => {
  it("takes the folder name from a Windows path", () => {
    expect(shortLibraryName("D:\\Books\\Science Fiction")).toBe("Science Fiction");
  });
  it("takes the folder name from a POSIX path", () => {
    expect(shortLibraryName("/home/me/Books/Fiction")).toBe("Fiction");
  });
  it("ignores a trailing separator", () => {
    expect(shortLibraryName("/home/me/Fiction/")).toBe("Fiction");
  });
  it("falls back to the whole path when there is no separator", () => {
    expect(shortLibraryName("Fiction")).toBe("Fiction");
  });
});

describe("recentLibraryEntries", () => {
  const paths = ["/a/Fiction", "/b/Research", "/c/Comics"];

  it("omits the library already open", () => {
    const labels = recentLibraryEntries(paths, "/b/Research").map((e) => e.label);
    expect(labels).toEqual(["Fiction", "Comics"]);
  });

  it("lists everything when the current library is not among them", () => {
    expect(recentLibraryEntries(paths, "/z/Other")).toHaveLength(3);
  });

  it("does not list the same path twice", () => {
    expect(recentLibraryEntries(["/a/Fiction", "/a/Fiction"], "")).toHaveLength(1);
  });

  // Two libraries can share a folder name; the label is allowed to
  // repeat, but the ids must differ or the menu picks the wrong one.
  it("keeps distinct ids for same-named libraries in different places", () => {
    const entries = recentLibraryEntries(["/a/Fiction", "/b/Fiction"], "");
    expect(entries.map((e) => e.label)).toEqual(["Fiction", "Fiction"]);
    expect(new Set(entries.map((e) => e.id)).size).toBe(2);
  });

  it("round-trips a path through its id", () => {
    for (const entry of recentLibraryEntries(paths, "")) {
      expect(pathFromLibraryId(entry.id)).toBe(entry.path);
    }
  });

  it("survives a path containing a colon", () => {
    const [entry] = recentLibraryEntries(["C:\\Books\\SF"], "");
    expect(pathFromLibraryId(entry.id)).toBe("C:\\Books\\SF");
  });
});

describe("pathFromLibraryId", () => {
  it("returns null for an action id", () => {
    expect(pathFromLibraryId("switch-library")).toBeNull();
  });
});
