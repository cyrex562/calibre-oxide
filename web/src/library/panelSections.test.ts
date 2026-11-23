import { describe, expect, it } from "vitest";

import { DEFAULT_SECTIONS, loadSections, saveSections, SECTIONS_KEY, toggleSection } from "./panelSections";

function memoryStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return { data, getItem: (k: string) => data[k] ?? null, setItem: (k: string, v: string) => void (data[k] = v) };
}

describe("panel sections", () => {
  it("defaults to everything open except the preview", () => {
    expect(loadSections(memoryStorage())).toEqual(DEFAULT_SECTIONS);
    expect(DEFAULT_SECTIONS.preview).toBe(false);
  });
  it("toggles one section without touching the others", () => {
    const next = toggleSection(DEFAULT_SECTIONS, "cover");
    expect(next.cover).toBe(false);
    expect(next.metadata).toBe(true);
  });
  it("round-trips through storage", () => {
    const s = memoryStorage();
    saveSections(toggleSection(DEFAULT_SECTIONS, "files"), s);
    expect(loadSections(s).files).toBe(false);
    expect(loadSections(s).cover).toBe(true);
  });
  it("ignores corrupt or partial stored state", () => {
    expect(loadSections(memoryStorage({ [SECTIONS_KEY]: "{nope" }))).toEqual(DEFAULT_SECTIONS);
    expect(loadSections(memoryStorage({ [SECTIONS_KEY]: JSON.stringify({ cover: false, files: "x" }) }))).toEqual({ ...DEFAULT_SECTIONS, cover: false });
  });
  it("survives storage that throws", () => {
    const bad = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    expect(loadSections(bad)).toEqual(DEFAULT_SECTIONS);
    expect(() => saveSections(DEFAULT_SECTIONS, bad)).not.toThrow();
    expect(loadSections(null)).toEqual(DEFAULT_SECTIONS);
  });
});
