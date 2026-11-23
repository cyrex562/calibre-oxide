import { describe, expect, it } from "vitest";

import { runPool } from "./pool";

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

describe("runPool", () => {
  it("runs every item with at most `concurrency` in flight", async () => {
    let running = 0;
    let peak = 0;
    const seen: number[] = [];
    const result = await runPool(
      Array.from({ length: 12 }, (_, i) => i),
      async (n) => {
        running += 1;
        peak = Math.max(peak, running);
        await tick();
        seen.push(n);
        running -= 1;
      },
      { concurrency: 4 },
    );
    expect(result).toEqual({ done: 12, skipped: 0 });
    expect(peak).toBe(4);
    expect(seen.sort((a, b) => a - b)).toEqual(Array.from({ length: 12 }, (_, i) => i));
  });

  it("reports progress as items finish", async () => {
    const progress: [number, number][] = [];
    await runPool([1, 2, 3], async () => tick(), { concurrency: 1, onProgress: (d, t) => progress.push([d, t]) });
    expect(progress).toEqual([[1, 3], [2, 3], [3, 3]]);
  });

  it("stops starting items once cancelled, and says how many were left", async () => {
    let cancel = false;
    let started = 0;
    const result = await runPool(
      Array.from({ length: 10 }, (_, i) => i),
      async () => {
        started += 1;
        if (started === 3) cancel = true;
        await tick();
      },
      { concurrency: 1, cancelled: () => cancel },
    );
    expect(started).toBe(3);
    expect(result).toEqual({ done: 3, skipped: 7 });
  });

  it("handles an empty list", async () => {
    expect(await runPool([], async () => undefined)).toEqual({ done: 0, skipped: 0 });
  });
});
