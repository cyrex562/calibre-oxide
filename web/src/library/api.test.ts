import { afterEach, describe, expect, it, vi } from "vitest";
import { addBook, fetchBooks, setCover, setFields } from "./api";
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

describe("addBook", () => {
  it("posts the raw file bytes to a job/filename-scoped URL and returns the parsed result", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ title: "T", authors: ["A"], languages: [], filename: "book.epub", id: "job1", book_id: 5 }),
    });
    vi.stubGlobal("fetch", fetchMock);

    const file = new File(["contents"], "book.epub");
    const result = await addBook(file);

    expect(result.book_id).toBe(5);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toMatch(/^\/cdb\/add-book\/[^/]+\/n\/book\.epub\/-$/);
    expect(init.method).toBe("POST");
    expect(init.body).toBe(file);
  });

  it("passes add_duplicates=y in the URL when requested", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({}) });
    vi.stubGlobal("fetch", fetchMock);

    await addBook(new File(["x"], "b.epub"), true);
    const [url] = fetchMock.mock.calls[0];
    expect(url).toContain("/y/b.epub/");
  });
});

describe("setFields", () => {
  it("wraps changes in a {changes} body and unwraps the id-keyed response", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ "1": bookStub(1) }),
    });
    vi.stubGlobal("fetch", fetchMock);

    const book = await setFields(1, { title: "New Title" });
    expect(book.id).toBe(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/set-fields/1");
    expect(JSON.parse(init.body)).toEqual({ changes: { title: "New Title" } });
  });
});

describe("setCover", () => {
  it("posts the raw image bytes to the book's set-cover URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => [1] });
    vi.stubGlobal("fetch", fetchMock);

    const file = new File(["jpeg bytes"], "cover.jpg");
    await setCover(1, file);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/set-cover/1");
    expect(init.body).toBe(file);
  });
});
