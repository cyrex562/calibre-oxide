import { afterEach, describe, expect, it, vi } from "vitest";
import { isTauri, tauriInvoke } from "./tauri";

afterEach(() => {
  // @ts-expect-error -- test-only cleanup of a global this module reads
  delete window.__TAURI_INTERNALS__;
});

describe("isTauri", () => {
  it("is false in a plain browser with no injected global", () => {
    expect(isTauri()).toBe(false);
  });

  it("is true once Tauri's own global is present", () => {
    // @ts-expect-error -- simulating Tauri's real injected global
    window.__TAURI_INTERNALS__ = { invoke: vi.fn() };
    expect(isTauri()).toBe(true);
  });
});

describe("tauriInvoke", () => {
  it("throws outside the desktop app rather than silently no-op'ing", async () => {
    await expect(tauriInvoke("choose_library")).rejects.toThrow(/desktop app/);
  });

  it("delegates to the real injected invoke function", async () => {
    const invoke = vi.fn().mockResolvedValue(42);
    // @ts-expect-error -- simulating Tauri's real injected global
    window.__TAURI_INTERNALS__ = { invoke };
    const result = await tauriInvoke("some_command", { a: 1 });
    expect(result).toBe(42);
    expect(invoke).toHaveBeenCalledWith("some_command", { a: 1 });
  });
});
