import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_KEYMAP, DEFAULT_TOOLBAR_PREFS, fetchProfile, forgetProfiles, KEYMAP_PROFILE, saveProfile, TOOLBAR_ACTIONS, TOOLBAR_PREFS_PROFILE } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
  forgetProfiles();
});

describe("fetchProfile", () => {
  it("returns the named profile out of the get-all response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ "library-prefs": { sort: "title" }, "reader-prefs": { fontSizePercent: 120 } }),
      }),
    );
    const prefs = await fetchProfile<{ sort: string }>("library-prefs");
    expect(prefs).toEqual({ sort: "title" });
  });

  it("returns null for a profile that hasn't been saved yet", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, json: async () => ({}) }));
    const prefs = await fetchProfile("reader-prefs");
    expect(prefs).toBeNull();
  });
});

describe("fetchProfile sharing", () => {
  const all = { "library-prefs": { sort: "title" }, "table-prefs": { view: "grid" } };

  it("asks the server once for any number of blobs read in the same page load", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => all });
    vi.stubGlobal("fetch", fetchMock);
    const [a, b, c, d] = await Promise.all([fetchProfile("library-prefs"), fetchProfile("table-prefs"), fetchProfile("keymap"), fetchProfile("toolbar-prefs")]);
    expect(a).toEqual({ sort: "title" });
    expect(b).toEqual({ view: "grid" });
    expect(c).toBeNull();
    expect(d).toBeNull();
    // Later reads (a second component, a remount) share it too.
    await fetchProfile("reader-prefs");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("does not remember a failure: the next read asks again", async () => {
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new TypeError("Failed to fetch"))
      .mockResolvedValue({ ok: true, json: async () => all });
    vi.stubGlobal("fetch", fetchMock);
    await expect(fetchProfile("library-prefs")).rejects.toThrow();
    expect(await fetchProfile("library-prefs")).toEqual({ sort: "title" });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("forgets what it shared once something is saved, so the next read sees the write", async () => {
    let stored: Record<string, unknown> = { "library-prefs": { sort: "title" } };
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/reader-profiles/save") {
        const body = JSON.parse(String(init?.body)) as { name: string; profile: unknown };
        stored = { ...stored, [body.name]: body.profile };
        return { ok: true, json: async () => true };
      }
      return { ok: true, json: async () => stored };
    });
    vi.stubGlobal("fetch", fetchMock);
    expect(await fetchProfile("library-prefs")).toEqual({ sort: "title" });
    await saveProfile("library-prefs", { sort: "authors" });
    expect(await fetchProfile("library-prefs")).toEqual({ sort: "authors" });
  });
});

describe("saveProfile", () => {
  it("posts the name and profile as a single JSON body", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => true });
    vi.stubGlobal("fetch", fetchMock);

    await saveProfile("library-prefs", { sort: "title", pageSize: 48 });

    expect(fetchMock).toHaveBeenCalledWith(
      "/reader-profiles/save",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ name: "library-prefs", profile: { sort: "title", pageSize: 48 } }),
      }),
    );
  });

  it("round-trips a rebound keymap through the same profile storage", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => true });
    vi.stubGlobal("fetch", fetchMock);

    const rebound = { ...DEFAULT_KEYMAP, readerNext: " " };
    await saveProfile(KEYMAP_PROFILE, rebound);

    expect(fetchMock).toHaveBeenCalledWith(
      "/reader-profiles/save",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ name: "keymap", profile: rebound }),
      }),
    );
  });

  it("round-trips a hidden/reordered toolbar layout through the same profile storage", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => true });
    vi.stubGlobal("fetch", fetchMock);

    const reordered = { hidden: ["fetch-news"], order: [...TOOLBAR_ACTIONS.map((a) => a.id)].reverse() };
    await saveProfile(TOOLBAR_PREFS_PROFILE, reordered);

    expect(fetchMock).toHaveBeenCalledWith(
      "/reader-profiles/save",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ name: "toolbar-prefs", profile: reordered }),
      }),
    );
  });

  it("DEFAULT_TOOLBAR_PREFS hides nothing and imposes no explicit order", () => {
    expect(DEFAULT_TOOLBAR_PREFS).toEqual({ hidden: [], order: [] });
  });
});
