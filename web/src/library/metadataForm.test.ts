import { describe, expect, it } from "vitest";

import { changesFromForm, cloneForm, draftCount, DraftStore, formFromBook, isFormDirty, rebaseForm, sharedDrafts } from "./metadataForm";
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

describe("sending only what was edited", () => {
  it("sends just the fields that differ from the baseline", () => {
    const base = formFromBook(book, [edition]);
    const form = { ...cloneForm(base), title: "Dune Messiah" };
    expect(changesFromForm(form, [edition], base)).toEqual({ title: "Dune Messiah" });
    const form2 = { ...cloneForm(base), custom: { edition: "Second" }, rating: 2 };
    expect(changesFromForm(form2, [edition], base)).toEqual({ rating: 2, edition: "Second" });
  });

  it("sends nothing for an untouched form, and everything without a baseline (the old behaviour)", () => {
    const base = formFromBook(book, [edition]);
    expect(changesFromForm(cloneForm(base), [edition], base)).toEqual({});
    expect(Object.keys(changesFromForm(cloneForm(base), [edition]))).toContain("authors");
  });

  it("does not read a number input handing back 1 for the string '1' as an edit", () => {
    const base = formFromBook(book, []);
    const form = cloneForm(base) as unknown as Record<string, unknown>;
    form.seriesIndex = 1;
    expect(changesFromForm(form as never, [], base)).toEqual({});
  });
});

describe("rebasing a form onto a re-read book", () => {
  it("lets untouched fields follow the new values and keeps the edited ones", () => {
    const oldBase = formFromBook(book, [edition]);
    const form = { ...cloneForm(oldBase), publisher: "Typed" };
    const reread = formFromBook({ ...book, title: "Edited in the table", edition: "Third" } as BookSummary, [edition]);
    const out = rebaseForm(form, oldBase, reread);
    expect(out.title).toBe("Edited in the table");
    expect(out.custom.edition).toBe("Third");
    expect(out.publisher).toBe("Typed");
    // ... so the next save sends only the publisher.
    expect(changesFromForm(out, [edition], reread)).toEqual({ publisher: "Typed" });
  });
});

describe("drafts that outlive the panel", () => {
  it("restores a draft onto the book's current values", () => {
    const store = new DraftStore();
    const base = formFromBook(book, []);
    store.stash(1, { ...cloneForm(base), publisher: "Typed" }, base);
    const fresh = formFromBook({ ...book, title: "Changed meanwhile" } as BookSummary, []);
    const restored = store.restore(1, fresh)!;
    expect(restored.title).toBe("Changed meanwhile");
    expect(restored.publisher).toBe("Typed");
    expect(store.restore(2, fresh)).toBeNull();
  });

  it("keeps a module-level store with a live count for the 'N unsaved drafts' indicator", () => {
    sharedDrafts.clear();
    const base = formFromBook(book, []);
    expect(draftCount.value).toBe(0);
    sharedDrafts.stash(1, { ...cloneForm(base), title: "a" }, base);
    sharedDrafts.stash(2, { ...cloneForm(base), title: "b" }, base);
    expect(draftCount.value).toBe(2);
    sharedDrafts.discard(1);
    expect(draftCount.value).toBe(1);
    sharedDrafts.stash(2, cloneForm(base), base); // edited back to the original
    expect(draftCount.value).toBe(0);
  });
});
