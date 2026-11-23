import { describe, expect, it } from "vitest";

import { activeFilters, classifyEmpty, folderOfQuery, type EmptyInputs } from "./emptyState";
import { folderQuery } from "./folders";

const base: EmptyInputs = { loading: false, error: null, total: 0, query: "", virtualLibrary: "", showMarkedOnly: false };

describe("classifyEmpty", () => {
  it("calls a library with no books and no filters empty, so the user is asked to add some", () => {
    expect(classifyEmpty(base)).toBe("empty-library");
  });

  it("says no-results, not empty-library, when a filter is hiding the books", () => {
    expect(classifyEmpty({ ...base, query: "dune" })).toBe("no-results");
    expect(classifyEmpty({ ...base, virtualLibrary: "Unread" })).toBe("no-results");
    expect(classifyEmpty({ ...base, showMarkedOnly: true })).toBe("no-results");
  });

  it("reports an error as an error, which gets a Retry", () => {
    expect(classifyEmpty({ ...base, error: "boom", query: "x" })).toBe("error");
  });

  it("is loading while the first list arrives, and says nothing once there are rows", () => {
    expect(classifyEmpty({ ...base, loading: true })).toBe("loading");
    expect(classifyEmpty({ ...base, total: 12, query: "x" })).toBe("none");
  });
});

describe("activeFilters", () => {
  it("lists each active filter with a label to show next to its clear button", () => {
    expect(activeFilters({ query: "dune", virtualLibrary: "Unread", showMarkedOnly: true })).toEqual([
      { id: "search", label: "Search: dune" },
      { id: "virtual-library", label: "Virtual library: Unread" },
      { id: "marked", label: "Marked books only" },
    ]);
  });

  it("recognises a folder selection by its query, and names the folder", () => {
    expect(activeFilters({ query: folderQuery("Authors/Ann Lee"), virtualLibrary: "", showMarkedOnly: false })).toEqual([{ id: "folder", label: "Folder: Authors/Ann Lee" }]);
    expect(folderOfQuery(folderQuery('He said "hi"'))).toBe('He said "hi"');
    expect(folderOfQuery("title:dune")).toBeNull();
  });

  it("has nothing for an unfiltered list", () => {
    expect(activeFilters({ query: "  ", virtualLibrary: "", showMarkedOnly: false })).toEqual([]);
  });
});
