import { afterEach, describe, expect, it, vi } from "vitest";
import { addBookmark, getAnnotations, removeAnnotation } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getAnnotations", () => {
  it("unwraps the book:fmt-keyed response down to its annotations_map", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ "1:epub": { annotations_map: { bookmark: [{ type: "bookmark", title: "Ch 1", timestamp: "t", pos: "p", pos_type: "lexmechanic-simple-pos" }] } } }),
    });
    vi.stubGlobal("fetch", fetchMock);

    const map = await getAnnotations("1", "epub");
    expect(map.bookmark).toHaveLength(1);
    expect(map.bookmark?.[0].title).toBe("Ch 1");
    expect(fetchMock.mock.calls[0][0]).toBe("/book-get-annotations/default/1-epub");
  });

  it("returns an empty object for a book with no annotations yet", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({}) });
    vi.stubGlobal("fetch", fetchMock);

    const map = await getAnnotations("1", "epub");
    expect(map).toEqual({});
  });
});

describe("addBookmark", () => {
  it("posts a real bookmark annotation and returns it", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);

    const bookmark = await addBookmark("1", "epub", "Chapter 1", "lexmechanic-simple-pos:2:");
    expect(bookmark.title).toBe("Chapter 1");
    expect(bookmark.type).toBe("bookmark");
    expect(bookmark.pos_type).toBe("lexmechanic-simple-pos");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/book-update-annotations/default/1/epub");
    const body = JSON.parse(init.body);
    expect(body.bookmark).toHaveLength(1);
    expect(body.bookmark[0].title).toBe("Chapter 1");
    expect(body.bookmark[0].pos).toBe("lexmechanic-simple-pos:2:");
  });

  it("throws on a real failure", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: "Not Found" });
    vi.stubGlobal("fetch", fetchMock);

    await expect(addBookmark("999", "epub", "x", "p")).rejects.toThrow(/404/);
  });
});

describe("removeAnnotation", () => {
  it("re-saves an EPUB bookmark as removed, with a newer timestamp", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    const bookmark = { type: "bookmark" as const, title: "Ch 1", timestamp: "2020-01-01T00:00:00.000Z", pos: "p", pos_type: "lexmechanic-simple-pos" as const };
    await removeAnnotation("7", "epub", "bookmark", bookmark);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/book-update-annotations/default/7/epub");
    const sent = JSON.parse(init.body).bookmark[0];
    expect(sent).toMatchObject({ title: "Ch 1", pos: "p", removed: true });
    expect(Date.parse(sent.timestamp)).toBeGreaterThan(Date.parse(bookmark.timestamp));
  });

  it("stays newer than a bookmark stamped in the future", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    const future = new Date(Date.now() + 3_600_000).toISOString();
    await removeAnnotation("7", "epub", "bookmark", { type: "bookmark", title: "x", timestamp: future, pos: "p", pos_type: "lexmechanic-simple-pos" });
    expect(Date.parse(JSON.parse(fetchMock.mock.calls[0][1].body).bookmark[0].timestamp)).toBeGreaterThan(Date.parse(future));
  });
});
