import { afterEach, describe, expect, it, vi } from "vitest";
import { addBook, addFormat, catalogDownloadUrl, deleteBooks, deleteSavedSearch, deleteVirtualLibrary, evaluateTemplate, fetchBooks, fetchConversionBookData, fetchDataFiles, fetchSavedSearches, ftsSearch, ftsSnippets, getConversionStatus, getNewsFetchStatus, removeDataFile, removeFormat, renameSavedSearch, setCover, setFields, setFtsEnabled, setSavedSearch, setVirtualLibrary, shareEmail, startConversion, startNewsFetch, uploadDataFile } from "./api";
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

describe("startNewsFetch", () => {
  it("posts the title and feed list, returning the bare job id", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => 7 });
    vi.stubGlobal("fetch", fetchMock);

    const jobId = await startNewsFetch("My Weekly", ["http://example.com/feed.xml"]);
    expect(jobId).toBe(7);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/news/fetch");
    expect(JSON.parse(init.body)).toEqual({ title: "My Weekly", feeds: ["http://example.com/feed.xml"] });
  });
});

describe("getNewsFetchStatus", () => {
  it("fetches the real status endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ running: false, ok: true, book_id: 5 }) });
    vi.stubGlobal("fetch", fetchMock);

    const status = await getNewsFetchStatus(7);
    expect(status.ok).toBe(true);
    expect(status.book_id).toBe(5);
    expect(fetchMock.mock.calls[0][0]).toBe("/news/status/7");
  });
});

describe("shareEmail", () => {
  it("posts the book/format/addresses/relay in one body", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ok: true }) });
    vi.stubGlobal("fetch", fetchMock);

    await shareEmail(1, "epub", "me@example.com", "you@example.com", { relay: "smtp.example.com", port: 587, encryption: "tls" }, "Subject");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/share/email");
    expect(JSON.parse(init.body)).toEqual({
      book_id: 1,
      format: "epub",
      from: "me@example.com",
      to: "you@example.com",
      subject: "Subject",
      relay: { relay: "smtp.example.com", port: 587, encryption: "tls" },
    });
  });
});

describe("fetchDataFiles / uploadDataFile / removeDataFile", () => {
  it("lists against the real single-library route shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ data_files: { "data/notes.pdf": { size: 42, mtime_ns: 1 } } }) });
    vi.stubGlobal("fetch", fetchMock);

    const files = await fetchDataFiles(7);

    expect(fetchMock).toHaveBeenCalledWith("/data-files/list/7/default", undefined);
    expect(files).toEqual({ "data/notes.pdf": { size: 42, mtime_ns: 1 } });
  });

  it("uploads a file as a data: URL and returns the updated list", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ error: "", data_files: { "data/a.txt": { size: 1, mtime_ns: 1 } } }) });
    vi.stubGlobal("fetch", fetchMock);
    const file = new File(["x"], "a.txt", { type: "text/plain" });

    const files = await uploadDataFile(7, file);

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/data-files/upload/7/default");
    const body = JSON.parse(init.body);
    expect(body).toHaveLength(1);
    expect(body[0].name).toBe("a.txt");
    expect(body[0].data_url).toMatch(/^data:/);
    expect(files).toEqual({ "data/a.txt": { size: 1, mtime_ns: 1 } });
  });

  it("throws the server's own error when upload fails", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, json: async () => ({ error: "boom", data_files: {} }) }));
    const file = new File(["x"], "a.txt");
    await expect(uploadDataFile(7, file)).rejects.toThrow("boom");
  });

  it("removes by relpath and returns the updated list", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ data_files: {} }) });
    vi.stubGlobal("fetch", fetchMock);

    const files = await removeDataFile(7, "data/a.txt");

    expect(fetchMock).toHaveBeenCalledWith("/data-files/remove/7/default", expect.objectContaining({ method: "POST", body: JSON.stringify(["data/a.txt"]) }));
    expect(files).toEqual({});
  });
});

describe("evaluateTemplate", () => {
  it("posts the template and returns the server's real ok/result shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ok: true, result: "My Title" }) });
    vi.stubGlobal("fetch", fetchMock);

    const result = await evaluateTemplate(7, "field('title')");

    expect(fetchMock).toHaveBeenCalledWith("/template-tester/evaluate/7/default", expect.objectContaining({ method: "POST", body: JSON.stringify({ template: "field('title')" }) }));
    expect(result).toEqual({ ok: true, result: "My Title" });
  });

  it("passes through a real ok:false/error result without throwing", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ok: false, error: "Interpreter: Unknown identifier 'nope' - line number 1" }) }));
    const result = await evaluateTemplate(7, "nope");
    expect(result.ok).toBe(false);
    expect(result.error).toContain("Unknown identifier");
  });
});
