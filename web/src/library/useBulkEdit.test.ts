// What Apply does in the bulk edit panel: validate before touching a book, send
// only the books that need a change, report what happened, and reset the panel
// (and the selection) only after a clean run.

import { ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { BookSummary, FieldMetaEntry } from "./types";
import { RelativeUnsupportedError } from "./bulkRelative";
import { useBulkEdit } from "./useBulkEdit";

const api = vi.hoisted(() => ({ setFields: vi.fn(), setFieldsBatch: vi.fn(), setFieldsRelative: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<typeof import("./api")>()), setFields: api.setFields, setFieldsBatch: api.setFieldsBatch, setFieldsRelative: api.setFieldsRelative }));

function book(id: number, tags: string[] = []): BookSummary {
  return { id, title: `Book ${id}`, authors: ["Ann Lee"], tags } as unknown as BookSummary;
}

function setup(ids: number[], books: BookSummary[]) {
  const selectedIds = ref(new Set(ids));
  const selectMode = ref(true);
  const fieldMetadata = ref<Record<string, FieldMetaEntry>>({});
  const byId = new Map(books.map((b) => [b.id, b]));
  const changed = vi.fn(async () => {});
  const bulk = useBulkEdit({
    selectedIds,
    selectMode,
    fieldMetadata,
    win: { ensureLoaded: async () => [], getById: (id) => byId.get(id) },
    onChanged: changed,
  });
  bulk.open = true;
  return { bulk, selectedIds, selectMode, fieldMetadata, changed };
}

beforeEach(() => {
  api.setFields.mockReset();
  api.setFieldsBatch.mockReset();
  // An older server: add/remove tags falls back to computing each book's list here.
  api.setFieldsRelative.mockReset();
  api.setFieldsRelative.mockRejectedValue(new RelativeUnsupportedError());
});

describe("useBulkEdit", () => {
  it("refuses an empty edit before sending anything", async () => {
    const { bulk, changed } = setup([1, 2], [book(1), book(2)]);
    await bulk.run();
    expect(bulk.errors).toEqual(["Nothing to apply -- fill in at least one field."]);
    expect(api.setFieldsBatch).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();
    expect(bulk.open).toBe(true);
  });

  it("refuses a bad regex before touching any book", async () => {
    const { bulk } = setup([1], [book(1)]);
    bulk.spec.searchReplace = { field: "title", find: "(", replace: "x", useRegex: true };
    await bulk.run();
    expect(bulk.errors).toHaveLength(1);
    expect(api.setFieldsBatch).not.toHaveBeenCalled();
  });

  it("does nothing with no selection", async () => {
    const { bulk } = setup([], []);
    bulk.spec.publisher = "Acme";
    await bulk.run();
    expect(api.setFieldsBatch).not.toHaveBeenCalled();
    expect(bulk.summary).toBeNull();
  });

  it("sends the books that need a change, then closes the panel and clears the selection", async () => {
    api.setFieldsBatch.mockImplementation(async (groups: { book_ids: number[] }[]) => ({ updated: groups.flatMap((g) => g.book_ids), failed: [] }));
    const { bulk, selectedIds, selectMode, changed } = setup([1, 2, 3], [book(1, ["a"]), book(2, ["b"]), book(3, ["scifi"])]);
    bulk.spec.tags = { mode: "add", value: "scifi" };
    await bulk.run();
    // Book 3 already has the tag: it is counted, not sent.
    expect(api.setFieldsBatch).toHaveBeenCalledTimes(1);
    const groups = api.setFieldsBatch.mock.calls[0][0] as { book_ids: number[] }[];
    expect(groups.flatMap((g) => g.book_ids).sort()).toEqual([1, 2]);
    expect(bulk.summary).toBe("Updated 2 book(s), 1 already matched.");
    expect(bulk.open).toBe(false);
    expect(selectMode.value).toBe(false);
    expect(selectedIds.value.size).toBe(0);
    expect(bulk.spec.tags?.value).toBe("");
    expect(changed).toHaveBeenCalledTimes(1);
    expect(bulk.busy).toBe(false);
    expect(bulk.progress).toBeNull();
  });

  it("adds tags through the server's add operation, reading no book first", async () => {
    api.setFieldsRelative.mockImplementation(async (ids: number[]) => ({ updated: ids.slice(0, 2), unchanged: ids.slice(2), failed: [] }));
    const ensureLoaded = vi.fn(async () => []);
    const selectedIds = ref(new Set([1, 2, 3]));
    const selectMode = ref(true);
    const changed = vi.fn(async () => {});
    const bulk = useBulkEdit({ selectedIds, selectMode, fieldMetadata: ref({}), win: { ensureLoaded, getById: () => undefined }, onChanged: changed });
    bulk.open = true;
    bulk.spec.tags = { mode: "add", value: "scifi, new" };
    await bulk.run();
    expect(api.setFieldsRelative).toHaveBeenCalledTimes(1);
    expect(api.setFieldsRelative.mock.calls[0]).toEqual([[1, 2, 3], { field: "tags", mode: "add", values: ["scifi", "new"] }]);
    expect(ensureLoaded).not.toHaveBeenCalled();
    expect(api.setFieldsBatch).not.toHaveBeenCalled();
    expect(bulk.summary).toBe("Updated 2 book(s), 1 already matched.");
    expect(bulk.open).toBe(false);
    expect(selectedIds.value.size).toBe(0);
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("removes tags through the server too, and keeps the panel open when a book fails", async () => {
    api.setFieldsRelative.mockResolvedValue({ updated: [1], failed: [{ book_id: 2, error: "locked" }] });
    const { bulk, selectedIds } = setup([1, 2], []);
    bulk.spec.tags = { mode: "remove", value: "old" };
    await bulk.run();
    expect(api.setFieldsRelative.mock.calls[0][1]).toEqual({ field: "tags", mode: "remove", values: ["old"] });
    expect(bulk.errors).toEqual(["Book 2: locked"]);
    expect(bulk.open).toBe(true);
    expect(selectedIds.value.size).toBe(2);
  });

  it("does not use the add operation when anything else is edited too", async () => {
    api.setFieldsBatch.mockImplementation(async (groups: { book_ids: number[] }[]) => ({ updated: groups.flatMap((g) => g.book_ids), failed: [] }));
    const { bulk } = setup([1], [book(1)]);
    bulk.spec.tags = { mode: "add", value: "x" };
    bulk.spec.publisher = "Acme";
    await bulk.run();
    expect(api.setFieldsRelative).not.toHaveBeenCalled();
    expect(api.setFieldsBatch).toHaveBeenCalledTimes(1);
  });

  it("keeps the panel and the selection open when a book fails", async () => {
    api.setFieldsBatch.mockResolvedValue({ updated: [1], failed: [{ book_id: 2, error: "locked" }] });
    const { bulk, selectedIds, changed } = setup([1, 2], [book(1), book(2)]);
    bulk.spec.publisher = "Acme";
    await bulk.run();
    expect(bulk.errors).toEqual(["Book 2: locked"]);
    expect(bulk.summary).toBe("Updated 1 book(s), 1 failed.");
    expect(bulk.open).toBe(true);
    expect(selectedIds.value.size).toBe(2);
    expect(bulk.spec.publisher).toBe("Acme");
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("reports a selected book that could not be loaded", async () => {
    api.setFieldsBatch.mockResolvedValue({ updated: [1], failed: [] });
    const { bulk } = setup([1, 2], [book(1)]);
    bulk.spec.publisher = "Acme";
    await bulk.run();
    expect(bulk.errors).toEqual(["Book 2: could not be loaded"]);
    expect(bulk.open).toBe(true);
  });

  it("applies a rating or series index only when its box is ticked", async () => {
    api.setFieldsBatch.mockResolvedValue({ updated: [1], failed: [] });
    const { bulk } = setup([1], [book(1)]);
    bulk.rating = 4;
    bulk.seriesIndex = 3;
    await bulk.run();
    expect(api.setFieldsBatch).not.toHaveBeenCalled();
    bulk.applyRating = true;
    await bulk.run();
    const groups = api.setFieldsBatch.mock.calls[0][0] as { changes: Record<string, unknown> }[];
    expect(groups[0].changes).toEqual({ rating: 4 });
  });

  it("offers the custom columns of the library", () => {
    const { bulk, fieldMetadata } = setup([1], [book(1)]);
    fieldMetadata.value = {
      "#shelf": { is_custom: true, label: "shelf", name: "Shelf" } as unknown as FieldMetaEntry,
      title: { is_custom: false, label: "title", name: "Title" } as unknown as FieldMetaEntry,
    };
    expect(bulk.customColumns.map((c) => c.label)).toEqual(["shelf"]);
  });
});
