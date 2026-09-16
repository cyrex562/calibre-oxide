import { afterEach, describe, expect, it, vi } from "vitest";
import { commitTweakSession, discardTweakSession, fetchTweakFile, openTweakSession, saveTweakFile } from "./tweak";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("openTweakSession", () => {
  it("opens against the real epub-only route shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ session_id: "abc", files: ["content.opf"] }) });
    vi.stubGlobal("fetch", fetchMock);

    const session = await openTweakSession(7);

    expect(fetchMock).toHaveBeenCalledWith("/tweak/open/7/epub/default", { method: "POST" });
    expect(session).toEqual({ session_id: "abc", files: ["content.opf"] });
  });
});

describe("fetchTweakFile / saveTweakFile", () => {
  it("percent-encodes each path segment of a nested file name individually", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, text: async () => "<p>hi</p>" });
    vi.stubGlobal("fetch", fetchMock);

    await fetchTweakFile("abc", "text/a b.xhtml");

    expect(fetchMock).toHaveBeenCalledWith("/tweak/file/abc/text/a%20b.xhtml");
  });

  it("saves raw text as the request body, not JSON", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, text: async () => "" });
    vi.stubGlobal("fetch", fetchMock);

    await saveTweakFile("abc", "content.opf", "<p>edited</p>");

    expect(fetchMock).toHaveBeenCalledWith("/tweak/file/abc/content.opf", { method: "POST", body: "<p>edited</p>" });
  });

  it("throws the server's own error text when the file is binary", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false, status: 400, statusText: "Bad Request", text: async () => "\"cover.png\" is a binary file" }));
    await expect(fetchTweakFile("abc", "cover.png")).rejects.toThrow("binary file");
  });
});

describe("commitTweakSession / discardTweakSession", () => {
  it("commit posts to the real route", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, text: async () => "" });
    vi.stubGlobal("fetch", fetchMock);
    await commitTweakSession("abc");
    expect(fetchMock).toHaveBeenCalledWith("/tweak/commit/abc", { method: "POST" });
  });

  it("discard posts to the real route and never throws (best-effort cleanup)", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: "Not Found" });
    vi.stubGlobal("fetch", fetchMock);
    await expect(discardTweakSession("abc")).resolves.toBeUndefined();
    expect(fetchMock).toHaveBeenCalledWith("/tweak/discard/abc", { method: "POST" });
  });
});
