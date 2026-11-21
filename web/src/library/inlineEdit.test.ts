import { describe, expect, it } from "vitest";

import { columnsFor, type BookColumn } from "./columns";
import { commitCellEdit, defaultEditKey, editKeyAction, initialText, isEditable, isEmptyCell, neighbourEditableKey, parseCellEdit, startsEditing, type EditTarget } from "./inlineEdit";
import type { BookFieldChanges, BookSummary, FieldMetaEntry } from "./types";

const meta = (o: Partial<FieldMetaEntry> & { label: string; datatype: string }): FieldMetaEntry => ({ key: `#${o.label}`, name: o.label, is_custom: true, is_editable: true, ...o }) as FieldMetaEntry;

const COLS = columnsFor(
  {
    "#notes": meta({ label: "notes", datatype: "text", is_multiple: { cache_to_list: ",", ui_to_list: ",", list_to_ui: ", " } }),
    "#kind": meta({ label: "kind", datatype: "text" }),
    "#mood": meta({ label: "mood", datatype: "enumeration", display: { enum_values: ["Happy", "Sad"] } }),
    "#pages": meta({ label: "pages", datatype: "int" }),
    "#score": meta({ label: "score", datatype: "float" }),
    "#read": meta({ label: "read", datatype: "bool" }),
    "#when": meta({ label: "when", datatype: "datetime" }),
    "#stars": meta({ label: "stars", datatype: "rating" }),
    "#calc": meta({ label: "calc", datatype: "composite" }),
    "#locked": meta({ label: "locked", datatype: "text", is_editable: false }),
  },
  [],
);
const col = (key: string): BookColumn => COLS.find((c) => c.key === key)!;

function book(over: Partial<BookSummary> = {}): BookSummary {
  return { id: 1, title: "A Title", authors: ["Ann Lee"], series: null, series_index: 1, tags: [], rating: null, pubdate: null, publisher: null, languages: [], formats: ["pdf"], timestamp: "2026-01-01T00:00:00+00:00", ...over } as unknown as BookSummary;
}

describe("which columns are editable", () => {
  it("lets the metadata columns be edited", () => {
    for (const k of ["title", "authors", "series", "series_index", "tags", "publisher", "rating", "pubdate", "languages"]) expect(isEditable(col(k)), k).toBe(true);
  });

  it("keeps computed and file columns read-only", () => {
    for (const k of ["formats", "timestamp", "last_modified", "id", "uuid", "author_sort", "identifiers", "comments", "calc", "locked"]) expect(isEditable(col(k)), k).toBe(false);
  });

  it("gives simple custom columns an editor of the right type", () => {
    expect(col("notes").editor).toEqual({ type: "list" });
    expect(col("kind").editor).toEqual({ type: "text" });
    expect(col("mood").editor).toEqual({ type: "enum", values: ["Happy", "Sad"] });
    expect(col("pages").editor).toEqual({ type: "number", integer: true });
    expect(col("score").editor).toEqual({ type: "number" });
    expect(col("read").editor).toEqual({ type: "bool" });
    expect(col("when").editor).toEqual({ type: "date" });
    expect(isEditable(col("stars"))).toBe(false); // scale cannot round-trip
  });

  it("picks title first, and walks editable neighbours only", () => {
    const row = [col("formats"), col("title"), col("timestamp"), col("authors"), col("size" as never) ?? col("id"), col("tags")].filter(Boolean);
    expect(defaultEditKey(row)).toBe("title");
    expect(neighbourEditableKey(row, "title", 1)).toBe("authors");
    expect(neighbourEditableKey(row, "authors", 1)).toBe("tags");
    expect(neighbourEditableKey(row, "tags", 1)).toBeNull();
    expect(neighbourEditableKey(row, "title", -1)).toBeNull();
    expect(neighbourEditableKey(row, "tags", -1)).toBe("authors");
  });
});

describe("empty cells and starting text", () => {
  it("treats missing values as empty", () => {
    expect(isEmptyCell(book({ series: null }), col("series"))).toBe(true);
    expect(isEmptyCell(book({ title: "" }), col("title"))).toBe(true);
    expect(isEmptyCell(book({ tags: [] }), col("tags"))).toBe(true);
    expect(isEmptyCell(book({ rating: 0 }), col("rating"))).toBe(true);
    expect(isEmptyCell(book(), col("title"))).toBe(false);
  });

  it("opens editors with the value in the form it is typed", () => {
    const b = book({ authors: ["A", "B"], tags: ["x", "y"], rating: 3.5, pubdate: "2020-05-06T00:00:00+00:00" });
    expect(initialText(b, col("authors"))).toBe("A & B");
    expect(initialText(b, col("tags"))).toBe("x, y");
    expect(initialText(b, col("rating"))).toBe("3.5");
    expect(initialText(b, col("pubdate"))).toBe("2020-05-06");
    expect(initialText(book({ pubdate: "0101-01-01T00:00:00+00:00" }), col("pubdate"))).toBe("");
    expect(initialText(book({ read: true } as never), col("read"))).toBe("true");
  });
});

describe("parsing typed text", () => {
  it("title: trims, refuses empty, ignores no-ops", () => {
    expect(parseCellEdit(col("title"), "  New  ", book())).toMatchObject({ status: "change", changes: { title: "New" } });
    expect(parseCellEdit(col("title"), "  ", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("title"), "A Title", book())).toEqual({ status: "unchanged" });
  });

  it("authors: splits on & like calibre", () => {
    expect(parseCellEdit(col("authors"), "Ann Lee & Bob Ray &  & Ann Lee", book({ authors: [] }))).toMatchObject({ changes: { authors: ["Ann Lee", "Bob Ray"] } });
    expect(parseCellEdit(col("authors"), "Ann Lee", book())).toEqual({ status: "unchanged" });
    expect(parseCellEdit(col("authors"), " & ", book())).toMatchObject({ status: "error" });
  });

  it("tags and languages: comma separated, de-duplicated", () => {
    expect(parseCellEdit(col("tags"), "a, b ,, a,c", book())).toMatchObject({ changes: { tags: ["a", "b", "c"] } });
    expect(parseCellEdit(col("tags"), "", book({ tags: ["x"] }))).toMatchObject({ changes: { tags: [] } });
    expect(parseCellEdit(col("tags"), "", book({ tags: [] }))).toEqual({ status: "unchanged" });
    expect(parseCellEdit(col("languages"), "EN, fra", book())).toMatchObject({ changes: { languages: ["en", "fra"] } });
  });

  it("custom multi-valued text is sent comma-joined", () => {
    const r = parseCellEdit(col("notes"), "a, b", book());
    expect(r).toMatchObject({ status: "change", changes: { notes: "a, b" }, patch: { notes: ["a", "b"] } });
  });

  it("series and publisher: plain text, empty clears", () => {
    expect(parseCellEdit(col("series"), "Dune", book())).toMatchObject({ changes: { series: "Dune" } });
    expect(parseCellEdit(col("publisher"), "", book({ publisher: "Acme" }))).toMatchObject({ changes: { publisher: "" }, patch: { publisher: null } });
  });

  it("series index: a number", () => {
    expect(parseCellEdit(col("series_index"), "2.5", book())).toMatchObject({ changes: { series_index: 2.5 } });
    expect(parseCellEdit(col("series_index"), "x", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("series_index"), "", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("series_index"), "1", book())).toEqual({ status: "unchanged" });
  });

  it("rating: 0 to 5 in halves, stars accepted, empty clears", () => {
    expect(parseCellEdit(col("rating"), "4", book())).toMatchObject({ changes: { rating: 4 } });
    expect(parseCellEdit(col("rating"), "3.3", book())).toMatchObject({ changes: { rating: 3.5 } });
    expect(parseCellEdit(col("rating"), "★★★½", book())).toMatchObject({ changes: { rating: 3.5 } });
    expect(parseCellEdit(col("rating"), "6", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("rating"), "abc", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("rating"), "", book({ rating: 4 }))).toMatchObject({ changes: { rating: 0 } });
    expect(parseCellEdit(col("rating"), "", book({ rating: null }))).toEqual({ status: "unchanged" });
  });

  it("dates: YYYY-MM-DD only, real dates only", () => {
    expect(parseCellEdit(col("pubdate"), "2024-02-29", book())).toMatchObject({ changes: { pubdate: "2024-02-29" } });
    expect(parseCellEdit(col("pubdate"), "2023-02-29", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("pubdate"), "May 5", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("pubdate"), "2020-05-06", book({ pubdate: "2020-05-06T00:00:00+00:00" }))).toEqual({ status: "unchanged" });
  });

  it("custom numbers, enum and bool", () => {
    expect(parseCellEdit(col("pages"), "12", book())).toMatchObject({ changes: { pages: 12 } });
    expect(parseCellEdit(col("pages"), "1.5", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("score"), "1.5", book())).toMatchObject({ changes: { score: 1.5 } });
    expect(parseCellEdit(col("mood"), "Sad", book())).toMatchObject({ changes: { mood: "Sad" } });
    expect(parseCellEdit(col("mood"), "Angry", book())).toMatchObject({ status: "error" });
    expect(parseCellEdit(col("read"), "true", book())).toMatchObject({ changes: { read: "true" }, patch: { read: true } });
    expect(parseCellEdit(col("read"), "maybe", book())).toMatchObject({ status: "error" });
  });

  it("refuses a read-only column", () => {
    expect(parseCellEdit(col("formats"), "x", book())).toMatchObject({ status: "error" });
  });
});

describe("key handling", () => {
  it("maps keys to editor actions", () => {
    expect(editKeyAction({ key: "Enter" })).toBe("save");
    expect(editKeyAction({ key: "Escape" })).toBe("cancel");
    expect(editKeyAction({ key: "Tab" })).toBe("next");
    expect(editKeyAction({ key: "Tab", shiftKey: true })).toBe("prev");
    expect(editKeyAction({ key: "a" })).toBeNull();
  });

  it("does not treat an IME-confirming Enter, or a shortcut chord, as a command", () => {
    expect(editKeyAction({ key: "Enter", isComposing: true })).toBeNull();
    expect(editKeyAction({ key: "Enter", ctrlKey: true })).toBeNull();
  });

  it("F2 starts editing", () => {
    expect(startsEditing({ key: "F2" })).toBe(true);
    expect(startsEditing({ key: "F3" })).toBe(false);
    expect(startsEditing({ key: "F2", ctrlKey: true })).toBe(false);
  });
});

describe("commit: optimistic update, save, rollback", () => {
  function target(initial: BookSummary): EditTarget & { rows: Map<number, BookSummary> } {
    const rows = new Map([[initial.id, initial]]);
    return {
      rows,
      getById: (id) => rows.get(id),
      patch: (id, c) => {
        const before = rows.get(id);
        if (before) rows.set(id, { ...before, ...c } as BookSummary);
        return before;
      },
      put: (b) => void rows.set(b.id, b),
    };
  }

  it("updates the row before the save returns, then adopts the server's copy", async () => {
    const t = target(book({ title: "" }));
    let seenDuringSave = "";
    const result = await commitCellEdit({
      target: t,
      bookId: 1,
      column: col("title"),
      raw: "Named",
      save: async (_id, changes: BookFieldChanges) => {
        seenDuringSave = t.rows.get(1)!.title;
        return book({ title: `${changes.title}!` });
      },
    });
    expect(seenDuringSave).toBe("Named");
    expect(result.status).toBe("saved");
    expect(t.rows.get(1)!.title).toBe("Named!");
  });

  it("does not call save for an unchanged value", async () => {
    const t = target(book());
    let calls = 0;
    const result = await commitCellEdit({ target: t, bookId: 1, column: col("title"), raw: "A Title", save: async () => ((calls += 1), book()) });
    expect(result).toEqual({ status: "unchanged" });
    expect(calls).toBe(0);
  });

  it("rolls the row back and reports the error when the save fails", async () => {
    const t = target(book({ title: "Original" }));
    const result = await commitCellEdit({
      target: t,
      bookId: 1,
      column: col("title"),
      raw: "Changed",
      save: async () => {
        throw new Error("disk full");
      },
    });
    expect(result).toEqual({ status: "error", message: "disk full" });
    expect(t.rows.get(1)!.title).toBe("Original");
  });

  it("reports a parse error without touching the row or saving", async () => {
    const t = target(book({ rating: 2 }));
    let calls = 0;
    const result = await commitCellEdit({ target: t, bookId: 1, column: col("rating"), raw: "9", save: async () => ((calls += 1), book()) });
    expect(result.status).toBe("error");
    expect(calls).toBe(0);
    expect(t.rows.get(1)!.rating).toBe(2);
  });

  it("fails cleanly for a book that is not loaded", async () => {
    const t = target(book());
    const result = await commitCellEdit({ target: t, bookId: 99, column: col("title"), raw: "x", save: async () => book() });
    expect(result.status).toBe("error");
  });
});
