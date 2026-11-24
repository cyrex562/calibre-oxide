import { describe, expect, it, vi } from "vitest";

import { clearStaleChunkReload, isStaleChunkError, reloadOnStaleChunk, STALE_CHUNK_MESSAGE } from "./staleChunk";

function memory() {
  const map = new Map<string, string>();
  return { getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v), removeItem: (k: string) => void map.delete(k) };
}

describe("the reload guard", () => {
  it("reloads a target once, then shows an error instead of looping", () => {
    const assign = vi.fn();
    const giveUp = vi.fn();
    const mem = memory();
    const err = new TypeError("Importing a module script failed.");
    expect(reloadOnStaleChunk(err, { fullPath: "/settings" }, assign, giveUp, mem, 1_000)).toBe(true);
    expect(assign).toHaveBeenCalledTimes(1);
    // the reloaded page fails the same way
    expect(reloadOnStaleChunk(err, { fullPath: "/settings" }, assign, giveUp, mem, 2_000)).toBe(true);
    expect(assign).toHaveBeenCalledTimes(1);
    expect(giveUp).toHaveBeenCalledWith(STALE_CHUNK_MESSAGE);
    // another target is its own first failure
    reloadOnStaleChunk(err, { fullPath: "/read/1/pdf" }, assign, giveUp, mem, 2_500);
    expect(assign).toHaveBeenCalledTimes(2);
    // after 30 s a reload is allowed again
    reloadOnStaleChunk(err, { fullPath: "/read/1/pdf" }, assign, giveUp, mem, 40_000);
    expect(assign).toHaveBeenCalledTimes(3);
    // and so is one after a successful navigation cleared the record
    clearStaleChunkReload(mem);
    reloadOnStaleChunk(err, { fullPath: "/read/1/pdf" }, assign, giveUp, mem, 40_100);
    expect(assign).toHaveBeenCalledTimes(4);
  });
});

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
