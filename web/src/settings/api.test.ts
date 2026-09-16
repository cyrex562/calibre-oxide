import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_KEYMAP, DEFAULT_TOOLBAR_PREFS, fetchProfile, KEYMAP_PROFILE, saveProfile, TOOLBAR_ACTIONS, TOOLBAR_PREFS_PROFILE } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
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
