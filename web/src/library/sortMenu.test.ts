import { describe, expect, it } from "vitest";

import { applySortChoice, primaryOf, sortFieldsOf, sortSummary, type SortState } from "./sortMenu";

const base: SortState = { sort: "timestamp", order: "desc" };
const label = (k: string) => ({ timestamp: "Date", title: "Title", authors: "Author" })[k] ?? k;

describe("sortFieldsOf", () => {
  it("splits and trims, ignoring empties", () => {
    expect(sortFieldsOf(" title , authors ,")).toEqual(["title", "authors"]);
    expect(sortFieldsOf("")).toEqual([]);
  });
});

describe("primaryOf", () => {
  it("is the first field", () => expect(primaryOf("title,authors")).toBe("title"));
  it("falls back when the string is empty", () => expect(primaryOf("")).toBe("timestamp"));
});

describe("applySortChoice", () => {
  it("sets the direction without touching the fields", () => {
    expect(applySortChoice({ sort: "title,authors", order: "desc" }, "dir:asc"))
      .toEqual({ sort: "title,authors", order: "asc" });
  });

  it("promotes a field to primary, keeping the rest in order", () => {
    expect(applySortChoice({ sort: "timestamp,title", order: "desc" }, "field:authors").sort)
      .toBe("authors,timestamp,title");
  });

  // Promoting something already used as a secondary sort must move it,
  // not duplicate it -- the server would otherwise sort by it twice.
  it("moves a secondary field to the front rather than duplicating it", () => {
    expect(applySortChoice({ sort: "timestamp,title", order: "desc" }, "field:title").sort)
      .toBe("title,timestamp");
  });

  it("appends a secondary field", () => {
    expect(applySortChoice(base, "add:title").sort).toBe("timestamp,title");
  });

  it("ignores appending a field already in the sort", () => {
    const state = { sort: "timestamp,title", order: "desc" } as SortState;
    expect(applySortChoice(state, "add:title")).toEqual(state);
  });

  it("drops a secondary field", () => {
    expect(applySortChoice({ sort: "timestamp,title", order: "desc" }, "drop:title").sort)
      .toBe("timestamp");
  });

  // An empty sort string lets the server choose, which reads as the
  // list spontaneously reordering itself.
  it("refuses to drop the last remaining field", () => {
    const state = { sort: "timestamp", order: "desc" } as SortState;
    expect(applySortChoice(state, "drop:timestamp")).toEqual(state);
  });

  it("returns the state unchanged for an unrecognised id", () => {
    expect(applySortChoice(base, "nonsense" as never)).toEqual(base);
  });
});

describe("sortSummary", () => {
  it("names one field and its direction", () => {
    expect(sortSummary(base, label)).toBe("Date (descending)");
  });

  it("chains secondary fields", () => {
    expect(sortSummary({ sort: "authors,title", order: "asc" }, label))
      .toBe("Author, then Title (ascending)");
  });

  it("describes an empty sort as the default rather than as nothing", () => {
    expect(sortSummary({ sort: "", order: "desc" }, label)).toBe("Date (descending)");
  });
});
