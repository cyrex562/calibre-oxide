import { afterEach, describe, expect, it, vi } from "vitest";
import { addBook, addFormat, catalogDownloadUrl, deleteBooks, deleteSavedSearch, deleteVirtualLibrary, fetchBooks, fetchConversionBookData, fetchSavedSearches, ftsSearch, ftsSnippets, getConversionStatus, removeFormat, renameSavedSearch, setCover, setFields, setFtsEnabled, setSavedSearch, setVirtualLibrary, startConversion } from "./api";
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

describe("deleteBooks", () => {
  it("posts a comma-joined id list to the delete-books URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({}) });
    vi.stubGlobal("fetch", fetchMock);

    await deleteBooks([1, 2, 3]);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/delete-books/1,2,3");
    expect(init.method).toBe("POST");
  });

  it("short-circuits without a network call for an empty id list", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await deleteBooks([]);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("addFormat", () => {
  it("base64-encodes the file as a data URL and sends it as added_formats", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ "1": bookStub(1) }) });
    vi.stubGlobal("fetch", fetchMock);

    const file = new File(["pdf bytes"], "extra.PDF");
    const book = await addFormat(1, file);
    expect(book.id).toBe(1);

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/set-fields/1");
    const body = JSON.parse(init.body);
    expect(body.changes.added_formats).toHaveLength(1);
    expect(body.changes.added_formats[0].ext).toBe("pdf");
    expect(body.changes.added_formats[0].data_url).toMatch(/^data:/);
  });
});

describe("removeFormat", () => {
  it("sends the extension as removed_formats", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ "1": bookStub(1) }) });
    vi.stubGlobal("fetch", fetchMock);

    await removeFormat(1, "pdf");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/cdb/set-fields/1");
    expect(JSON.parse(init.body)).toEqual({ changes: { removed_formats: ["pdf"] } });
  });
});

describe("fetchConversionBookData", () => {
  it("fetches the real book-data endpoint", async () => {
    const responseBody = { book_id: 1, title: "T", authors: ["A"], input_formats: ["EPUB"], output_formats: ["EPUB", "MOBI"] };
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => responseBody });
    vi.stubGlobal("fetch", fetchMock);

    const data = await fetchConversionBookData(1);
    expect(data).toEqual(responseBody);
    expect(fetchMock.mock.calls[0][0]).toBe("/conversion/book-data/1");
  });
});

describe("startConversion", () => {
  it("posts input/output formats and returns the bare job id", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => 42 });
    vi.stubGlobal("fetch", fetchMock);

    const jobId = await startConversion(1, "EPUB", "MOBI");
    expect(jobId).toBe(42);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/conversion/start/1");
    expect(JSON.parse(init.body)).toEqual({ input_fmt: "EPUB", output_fmt: "MOBI" });
  });
});

describe("getConversionStatus", () => {
  it("fetches the real status endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ running: false, ok: true, size: 123, fmt: "mobi" }) });
    vi.stubGlobal("fetch", fetchMock);

    const status = await getConversionStatus(42);
    expect(status.running).toBe(false);
    expect(status.ok).toBe(true);
    expect(fetchMock.mock.calls[0][0]).toBe("/conversion/status/42");
  });
});

describe("ftsSearch", () => {
  it("returns enabled:true with the parsed result on a real 200", async () => {
    const responseBody = { metadata: { "1": { title: "T", authors: "A" } }, indexing_status: { left: 0, total: 1 }, results: [{ book_id: 1, format: "EPUB" }] };
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 200, json: async () => responseBody });
    vi.stubGlobal("fetch", fetchMock);

    const outcome = await ftsSearch("rust");
    expect(outcome.enabled).toBe(true);
    if (outcome.enabled) expect(outcome.result).toEqual(responseBody);
    expect(fetchMock.mock.calls[0][0]).toBe("/fts/search?query=rust");
  });

  it("returns enabled:false on a real 428 (Precondition Required), without throwing", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 428 });
    vi.stubGlobal("fetch", fetchMock);

    const outcome = await ftsSearch("rust");
    expect(outcome).toEqual({ enabled: false });
  });

  it("still throws on a real, unrelated failure", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 500, statusText: "Internal Server Error" });
    vi.stubGlobal("fetch", fetchMock);

    await expect(ftsSearch("rust")).rejects.toThrow(/500/);
  });
});

describe("ftsSnippets", () => {
  it("fetches snippets for the given book ids and unwraps the snippets field", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ snippets: { "1": [{ formats: ["EPUB"], text: "a snippet" }] } }) });
    vi.stubGlobal("fetch", fetchMock);

    const snippets = await ftsSnippets([1], "rust");
    expect(snippets["1"][0].text).toBe("a snippet");
    expect(fetchMock.mock.calls[0][0]).toBe("/fts/snippets/1?query=rust");
  });

  it("short-circuits without a network call for an empty id list", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const snippets = await ftsSnippets([], "rust");
    expect(snippets).toEqual({});
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("setFtsEnabled", () => {
  it("posts the bare boolean body", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);

    await setFtsEnabled(true);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/fts/indexing");
    expect(init.body).toBe("true");
  });
});

describe("virtual library management", () => {
  it("setVirtualLibrary posts the query body to the name-scoped URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);

    await setVirtualLibrary("My VL", "tags:scifi");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/vl/set/My%20VL");
    expect(JSON.parse(init.body)).toEqual({ query: "tags:scifi" });
  });

  it("deleteVirtualLibrary posts with no body", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);

    await deleteVirtualLibrary("My VL");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/vl/delete/My%20VL");
    expect(init.method).toBe("POST");
  });

  it("setVirtualLibrary throws on a real failure", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 500, statusText: "Internal Server Error" });
    vi.stubGlobal("fetch", fetchMock);
    await expect(setVirtualLibrary("x", "y")).rejects.toThrow(/500/);
  });
});

describe("saved search management", () => {
  it("fetchSavedSearches fetches the real map", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ "My Search": "authors:asimov" }) });
    vi.stubGlobal("fetch", fetchMock);

    const map = await fetchSavedSearches();
    expect(map["My Search"]).toBe("authors:asimov");
    expect(fetchMock.mock.calls[0][0]).toBe("/ajax/saved-searches");
  });

  it("setSavedSearch posts the query body to the name-scoped URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);

    await setSavedSearch("My Search", "authors:asimov");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/saved-search/set/My%20Search");
    expect(JSON.parse(init.body)).toEqual({ query: "authors:asimov" });
  });

  it("deleteSavedSearch posts with no body", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    await deleteSavedSearch("My Search");
    expect(fetchMock.mock.calls[0][0]).toBe("/saved-search/delete/My%20Search");
  });

  it("renameSavedSearch posts to the old/new-name-scoped URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    await renameSavedSearch("Old", "New");
    expect(fetchMock.mock.calls[0][0]).toBe("/saved-search/rename/Old/New");
  });
});

describe("catalogDownloadUrl", () => {
  it("has no query string for the whole library", () => {
    expect(catalogDownloadUrl("")).toBe("/catalog/generate");
  });

  it("scopes to a search query when given one", () => {
    expect(catalogDownloadUrl("tags:scifi")).toBe("/catalog/generate?search=tags%3Ascifi");
  });
});
