import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchNoteByName, saveNote } from "./notes";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("fetchNoteByName", () => {
  it("resolves by display name against the real route shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ item_id: 7, html: "<p>hi</p>" }) });
    vi.stubGlobal("fetch", fetchMock);

    const note = await fetchNoteByName("authors", "Jane Doe");

    expect(fetchMock).toHaveBeenCalledWith("/get-note-from-item-val/authors/Jane%20Doe/default");
    expect(note).toEqual({ item_id: 7, html: "<p>hi</p>" });
  });

  it("throws on a non-ok response", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: "Not Found" }));
    await expect(fetchNoteByName("authors", "Nobody")).rejects.toThrow("404");
  });
});

describe("saveNote", () => {
  it("posts html and images, and returns the raw HTML response body (not JSON)", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, text: async () => "<p>saved</p>" });
    vi.stubGlobal("fetch", fetchMock);

    const result = await saveNote("authors", 7, "<p>edited</p>", { "/get-note-resource/a/b": { data: "/get-note-resource/a/b" } });

    expect(fetchMock).toHaveBeenCalledWith(
      "/set-note/authors/7/default",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ html: "<p>edited</p>", images: { "/get-note-resource/a/b": { data: "/get-note-resource/a/b" } } }),
      }),
    );
    expect(result).toBe("<p>saved</p>");
  });
});
