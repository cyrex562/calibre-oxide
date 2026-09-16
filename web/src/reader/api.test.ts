import { afterEach, describe, expect, it, vi } from "vitest";
import { addBookmark, getAnnotations } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getAnnotations", () => {
  it("unwraps the book:fmt-keyed response down to its annotations_map", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ "1:epub": { annotations_map: { bookmark: [{ type: "bookmark", title: "Ch 1", timestamp: "t", pos: "p", pos_type: "calibre-oxide-simple-pos" }] } } }),
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

    const bookmark = await addBookmark("1", "epub", "Chapter 1", "calibre-oxide-simple-pos:2:");
    expect(bookmark.title).toBe("Chapter 1");
    expect(bookmark.type).toBe("bookmark");
    expect(bookmark.pos_type).toBe("calibre-oxide-simple-pos");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/book-update-annotations/default/1/epub");
    const body = JSON.parse(init.body);
    expect(body.bookmark).toHaveLength(1);
    expect(body.bookmark[0].title).toBe("Chapter 1");
    expect(body.bookmark[0].pos).toBe("calibre-oxide-simple-pos:2:");
  });

  it("throws on a real failure", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: "Not Found" });
    vi.stubGlobal("fetch", fetchMock);

    await expect(addBookmark("999", "epub", "x", "p")).rejects.toThrow(/404/);
  });
});
