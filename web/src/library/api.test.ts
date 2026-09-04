import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchBooks } from "./api";
import type { BookSummary } from "./types";

function bookStub(id: number): BookSummary {
  return {
    id,
    title: `Book ${id}`,
    authors: [],
    rating: null,
    cover: `/get/cover/${id}`,
    thumbnail: `/get/thumb/${id}`,
    formats: [],
    main_format: null,
    other_formats: {},
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("fetchBooks", () => {
  it("re-orders the id-keyed response back into the requested id order", async () => {
    // /ajax/books returns an unordered {id: book|null} object -- the
    // page's own sort order (established server-side by /ajax/search)
    // lives only in the `ids` array the caller passes in.
    const responseBody: Record<string, BookSummary | null> = {
      "3": bookStub(3),
      "1": bookStub(1),
      "2": bookStub(2),
    };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => responseBody,
      }),
    );

    const books = await fetchBooks([1, 2, 3]);
    expect(books.map((b) => b.id)).toEqual([1, 2, 3]);
  });

  it("drops ids the server maps to null (e.g. a deleted book)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ "1": bookStub(1), "2": null }),
      }),
    );

    const books = await fetchBooks([1, 2]);
    expect(books.map((b) => b.id)).toEqual([1]);
  });

  it("short-circuits without a network call for an empty id list", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const books = await fetchBooks([]);
    expect(books).toEqual([]);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
