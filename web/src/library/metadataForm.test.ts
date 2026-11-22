import { describe, expect, it } from "vitest";

import { changesFromForm, cloneForm, DraftStore, formFromBook, isFormDirty } from "./metadataForm";
import type { BookSummary, FieldMetaEntry } from "./types";

const edition = { key: "#edition", label: "edition", name: "Edition", datatype: "text", is_custom: true, is_editable: true } as FieldMetaEntry;
const flag = { key: "#flag", label: "flag", name: "Flag", datatype: "bool", is_custom: true, is_editable: true } as FieldMetaEntry;

const book = {
  id: 1,
  title: "Dune",
  authors: ["Frank Herbert"],
  series: "Dune",
  series_index: 1,
  tags: ["scifi", "classic"],
  rating: 4,
  publisher: "Chilton",
  pubdate: "1965-08-01T00:00:00+00:00",
  comments: "Spice",
  edition: "First",
  flag: "1",
  cover: "",
  thumbnail: "",
  formats: [],
  main_format: null,
  other_formats: {},
} as BookSummary;

describe("metadata form", () => {
  it("fills every field, including custom columns", () => {
    const f = formFromBook(book, [edition, flag]);
    expect(f).toMatchObject({ title: "Dune", authors: "Frank Herbert", seriesIndex: "1", tags: "scifi, classic", pubdate: "1965-08-01", comments: "Spice" });
    expect(f.custom).toEqual({ edition: "First", flag: "true" });
  });
  it("is clean until something changes", () => {
    const base = formFromBook(book, [edition]);
    const form = cloneForm(base);
    expect(isFormDirty(form, base)).toBe(false);
    form.title = "Dune Messiah";
    expect(isFormDirty(form, base)).toBe(true);
    form.title = "Dune";
    expect(isFormDirty(form, base)).toBe(false);
    form.custom.edition = "Second";
    expect(isFormDirty(form, base)).toBe(true);
  });
  it("builds the same save request the old edit mode sent", () => {
    const form = formFromBook(book, [edition]);
    form.authors = "A & B";
    form.tags = " x , y ,";
    const changes = changesFromForm(form, [edition]);
    expect(changes).toMatchObject({ title: "Dune", authors: ["A", "B"], tags: ["x", "y"], series_index: 1, pubdate: "1965-08-01", edition: "First" });
  });
  it("leaves out an empty date and index", () => {
    const form = formFromBook({ ...book, pubdate: null, series_index: null }, []);
    const changes = changesFromForm(form, []);
    expect("pubdate" in changes).toBe(false);
    expect("series_index" in changes).toBe(false);
  });
  it("skips read-only custom columns", () => {
    const ro = { ...edition, is_editable: false } as FieldMetaEntry;
    expect("edition" in changesFromForm(formFromBook(book, [ro]), [ro])).toBe(false);
  });
});

describe("drafts", () => {
  it("keeps a dirty form per book and restores it", () => {
    const store = new DraftStore();
    const base = formFromBook(book, []);
    const form = { ...cloneForm(base), title: "Changed" };
    store.stash(1, form, base);
    expect(store.has(1)).toBe(true);
    expect(store.take(1)?.title).toBe("Changed");
    expect(store.has(2)).toBe(false);
  });
  it("forgets a draft that was edited back to the original, or discarded", () => {
    const store = new DraftStore();
    const base = formFromBook(book, []);
    store.stash(1, { ...base, title: "x" }, base);
    store.stash(1, cloneForm(base), base);
    expect(store.has(1)).toBe(false);
    store.stash(1, { ...base, title: "x" }, base);
    store.discard(1);
    expect(store.take(1)).toBeNull();
  });
});

describe("number inputs", () => {
  it("copes with v-model handing back numbers for string fields", () => {
    const base = formFromBook(book, [edition]);
    const form = cloneForm(base) as unknown as Record<string, unknown>;
    form.seriesIndex = 1; // what a number input produces for "1"
    expect(isFormDirty(form as never, base)).toBe(false);
    form.seriesIndex = 2.5;
    expect(isFormDirty(form as never, base)).toBe(true);
    expect(changesFromForm(form as never, []).series_index).toBe(2.5);
  });
});
