// C1: fast page turning must never leave a blank canvas or PDF.js's
// "Cannot use the same canvas during multiple render() operations".

import { describe, expect, it } from "vitest";

import { isRenderCancelled, LatestRenderer, PDF_LOAD_OPTIONS, RenderCancelled, renderPage } from "./pdf";

/** A PDF.js render task that rejects like the real one when cancelled. */
function fakeTask(log: string[], page: number, active: { count: number; max: number }) {
  let cancelled = false;
  let finish: () => void = () => {};
  active.count += 1;
  active.max = Math.max(active.max, active.count);
  const promise = new Promise<void>((resolve, reject) => {
    finish = () => {
      active.count -= 1;
      if (cancelled) reject(Object.assign(new Error("cancelled"), { name: "RenderingCancelledException" }));
      else {
        log.push(`drew ${page}`);
        resolve();
      }
    };
    setTimeout(finish, 5);
  });
  return {
    promise,
    cancel() {
      cancelled = true;
    },
  };
}

describe("LatestRenderer", () => {
  it("never runs two renders at once and ends on the newest page", async () => {
    const renderer = new LatestRenderer();
    const log: string[] = [];
    const active = { count: 0, max: 0 };

    const show = (page: number) =>
      renderer.run(async ({ onTask }) => {
        const task = fakeTask(log, page, active);
        onTask(task);
        await task.promise;
      });

    // Five Next clicks, a few milliseconds apart.
    const runs: Promise<boolean>[] = [];
    for (let page = 2; page <= 6; page += 1) {
      runs.push(show(page));
      await new Promise((r) => setTimeout(r, 1));
    }
    const results = await Promise.all(runs);

    expect(active.max).toBe(1);
    expect(log[log.length - 1]).toBe("drew 6");
    expect(results[results.length - 1]).toBe(true);
    // Everything before the newest was cancelled or dropped, not an error.
    expect(results.slice(0, -1).every((r) => r === false)).toBe(true);
  });

  it("does not start a request that was replaced while it waited", async () => {
    const renderer = new LatestRenderer();
    const started: number[] = [];
    const first = renderer.run(async () => {
      started.push(1);
      await new Promise((r) => setTimeout(r, 10));
    });
    const second = renderer.run(async () => void started.push(2));
    const third = renderer.run(async () => void started.push(3));
    await Promise.all([first, second, third]);
    // Only the newest ever starts: the first two were replaced before they began.
    expect(started).toEqual([3]);
  });

  it("reports a real failure but swallows a cancellation", async () => {
    const renderer = new LatestRenderer();
    await expect(
      renderer.run(async () => {
        throw new Error("bad pdf");
      }),
    ).rejects.toThrow("bad pdf");
    await expect(
      renderer.run(async () => {
        throw new RenderCancelled();
      }),
    ).resolves.toBe(false);
  });

  it("cancel() stops the running task and invalidates what waits", async () => {
    const renderer = new LatestRenderer();
    let cancelled = false;
    const running = renderer.run(async ({ onTask }) => {
      onTask({ cancel: () => (cancelled = true) });
      await new Promise((r) => setTimeout(r, 5));
    });
    await new Promise((r) => setTimeout(r, 0)); // let it start
    renderer.cancel();
    expect(cancelled).toBe(true);
    expect(await running).toBe(false);
  });
});

describe("renderPage", () => {
  it("abandons a page it was superseded on before touching the canvas", async () => {
    const canvas = document.createElement("canvas");
    const render = () => ({ promise: Promise.resolve(), cancel() {} });
    const doc = { getPage: async () => ({ getViewport: () => ({ width: 100, height: 100 }), render }) };
    await expect(renderPage(doc as never, 1, { canvas, scale: 1, isStale: () => true })).rejects.toSatisfy(isRenderCancelled);
    expect(canvas.width).toBe(300); // jsdom's untouched default
  });
});

describe("PDF_LOAD_OPTIONS", () => {
  // Range requests make page 1 of a big NAS PDF appear quickly; the
  // auto-fetch is what would otherwise pull the whole file anyway.
  it("keeps range requests on and stops background fetching", () => {
    expect(PDF_LOAD_OPTIONS.disableRange).toBe(false);
    expect(PDF_LOAD_OPTIONS.disableStream).toBe(false);
    expect(PDF_LOAD_OPTIONS.disableAutoFetch).toBe(true);
  });
});
