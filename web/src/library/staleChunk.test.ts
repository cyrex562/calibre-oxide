import { describe, expect, it, vi } from "vitest";

import { isStaleChunkError, reloadOnStaleChunk } from "./staleChunk";

describe("a lazy route whose script is gone", () => {
  it("is recognised in the wording of each browser", () => {
    expect(isStaleChunkError(new TypeError("Failed to fetch dynamically imported module: http://x/assets/ReaderView-abc.js"))).toBe(true);
    expect(isStaleChunkError(new TypeError("error loading dynamically imported module: http://x/a.js"))).toBe(true);
    expect(isStaleChunkError(new TypeError("Importing a module script failed."))).toBe(true);
    expect(isStaleChunkError(new Error("something else"))).toBe(false);
    expect(isStaleChunkError(undefined)).toBe(false);
  });

  it("reloads on the page the user was going to, and leaves other errors alone", () => {
    const assign = vi.fn();
    expect(reloadOnStaleChunk(new TypeError("Importing a module script failed."), { fullPath: "/read/7/pdf" }, assign)).toBe(true);
    expect(assign).toHaveBeenCalledWith("/read/7/pdf");
    assign.mockClear();
    expect(reloadOnStaleChunk(new Error("boom"), { fullPath: "/settings" }, assign)).toBe(false);
    expect(assign).not.toHaveBeenCalled();
  });
});
