import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applySavedSkin, applySkin, DEFAULT_SKIN, getSkin, setSkin, SKIN_KEY } from "./skin";

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-skin");
});
afterEach(() => vi.restoreAllMocks());

describe("skin preference", () => {
  it("defaults to classic when nothing is saved", () => {
    expect(DEFAULT_SKIN).toBe("classic");
    expect(getSkin()).toBe("classic");
  });

  it("applies the saved skin to <html> at start-up", () => {
    expect(applySavedSkin()).toBe("classic");
    expect(document.documentElement.getAttribute("data-skin")).toBe("classic");
    localStorage.setItem(SKIN_KEY, "modern");
    expect(applySavedSkin()).toBe("modern");
    expect(document.documentElement.getAttribute("data-skin")).toBe("modern");
  });

  it("persists modern, and reads it back", () => {
    setSkin("modern");
    expect(localStorage.getItem(SKIN_KEY)).toBe("modern");
    expect(getSkin()).toBe("modern");
    expect(document.documentElement.getAttribute("data-skin")).toBe("modern");
    setSkin("classic");
    expect(getSkin()).toBe("classic");
  });

  it("ignores a stored value it does not know", () => {
    localStorage.setItem(SKIN_KEY, "neon");
    expect(getSkin()).toBe("classic");
  });

  it("still applies the skin when storage refuses the write or the read", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("full");
    });
    setSkin("modern");
    expect(document.documentElement.getAttribute("data-skin")).toBe("modern");
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(getSkin()).toBe("classic");
  });

  it("applySkin only sets the attribute", () => {
    applySkin("modern");
    expect(localStorage.getItem(SKIN_KEY)).toBeNull();
  });
});
