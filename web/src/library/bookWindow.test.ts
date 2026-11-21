import { describe, expect, it } from "vitest";

import { createBookWindow, type IdList } from "./bookWindow";
import type { BookSummary } from "./types";

function summary(id: number, title = `Book ${id}`): BookSummary {
  return { id, title, authors: [] } as unknown as BookSummary;
}

/** A controllable fake server: each fetchBooks call waits until released. */
function harness(count: number, options: { chunkSize?: number; maxInFlight?: number } = {}) {
  let list: IdList = { ids: Array.from({ length: count }, (_, i) => i + 1), total: count };
  const calls: number[][] = [];
  const waiting: (() => void)[] = [];
  let titles: Record<number, string> = {};
  let manual = false;
  let idCalls = 0;
  const win = createBookWindow({
    chunkSize: options.chunkSize ?? 100,
    maxInFlight: options.maxInFlight ?? 2,
    fetchIds: async () => {
      idCalls += 1;
      return list;
    },
    fetchBooks: async (ids) => {
      calls.push(ids);
      if (manual) await new Promise<void>((r) => waiting.push(r));
      return ids.map((id) => summary(id, titles[id]));
    },
  });
  return {
    win,
    calls,
    setList: (ids: number[]) => (list = { ids, total: ids.length }),
    setTitles: (t: Record<number, string>) => (titles = t),
    hold: () => (manual = true),
    release: async () => {
      const w = waiting.splice(0);
      w.forEach((r) => r());
      await new Promise((r) => setTimeout(r, 0));
    },
    idCalls: () => idCalls,
  };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("loading the id list", () => {
  it("knows every id and the total without fetching any summary", async () => {
    const h = harness(9767);
    await h.win.reset();
    expect(h.win.total.value).toBe(9767);
    expect(h.win.ids.value).toHaveLength(9767);
    expect(h.calls).toHaveLength(0);
  });

  it("answers index and id lookups over rows that were never loaded", async () => {
    const h = harness(1000);
    await h.win.reset();
    expect(h.win.idAt(500)).toBe(501);
    expect(h.win.indexOf(501)).toBe(500);
    expect(h.win.indexOf(99999)).toBe(-1);
    expect(h.win.bookAt(500)).toBeUndefined();
  });
});

describe("chunked loading", () => {
  it("fetches only the chunks the range touches, aligned to chunk boundaries", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(95, 130); // chunks 0 and 1
    await tick();
    expect(h.calls.map((c) => [c[0], c[c.length - 1]])).toEqual([
      [1, 100],
      [101, 200],
    ]);
    expect(h.win.bookAt(120)?.id).toBe(121);
    expect(h.win.bookAt(250)).toBeUndefined();
  });

  it("does not refetch chunks that are already loaded", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(0, 50);
    await tick();
    h.win.setRange(150, 250);
    await tick();
    const before = h.calls.length;
    h.win.setRange(0, 50); // scroll back
    h.win.setRange(10, 60);
    await tick();
    expect(h.calls.length).toBe(before);
  });

  it("clamps a range past the end of the list", async () => {
    const h = harness(120);
    await h.win.reset();
    h.win.setRange(100, 5000);
    await tick();
    expect(h.calls.map((c) => c.length)).toEqual([20]);
  });

  it("does not queue requests for ranges the viewport has already left", async () => {
    const h = harness(10000, { maxInFlight: 1 });
    await h.win.reset();
    h.hold();
    h.win.setRange(0, 50); // chunk 0 starts
    for (let start = 1000; start < 9000; start += 1000) h.win.setRange(start, start + 50); // a fast scroll
    h.win.setRange(9000, 9050); // where it stops
    await h.release();
    await h.release();
    await h.release();
    const starts = h.calls.map((c) => c[0]);
    // chunk 0 was already in flight; of the rest only the final stop was ever requested.
    expect(starts).toEqual([1, 9001]);
  });

  it("limits requests in flight", async () => {
    const h = harness(10000, { maxInFlight: 2 });
    await h.win.reset();
    h.hold();
    h.win.setRange(0, 450); // 5 chunks
    expect(h.win.pendingCount()).toBe(2);
    await h.release();
    await h.release();
    await h.release();
    expect(h.calls).toHaveLength(5);
  });
});

describe("refresh", () => {
  it("keeps loaded rows visible until their replacements arrive", async () => {
    const h = harness(300);
    await h.win.reset();
    h.win.setRange(0, 50);
    await tick();
    expect(h.win.getById(1)?.title).toBe("Book 1");

    h.setTitles({ 1: "Edited" });
    h.hold();
    const done = h.win.refresh();
    await tick();
    // The list is re-read, but the row on screen is still there, not a placeholder.
    expect(h.win.getById(1)?.title).toBe("Book 1");
    expect(h.win.bookAt(0)).toBeDefined();
    await h.release();
    await done;
    await h.release();
    expect(h.win.getById(1)?.title).toBe("Edited");
  });

  it("re-reads only the chunks that are wanted, not the whole list", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(0, 50);
    await tick();
    h.win.setRange(500, 550);
    await tick();
    const before = h.calls.length;
    await h.win.refresh();
    await tick();
    expect(h.calls.length - before).toBe(1);
    expect(h.calls[h.calls.length - 1][0]).toBe(501);
  });

  it("follows rows that moved when the list changed, and drops ones that left", async () => {
    const h = harness(5, { chunkSize: 10 });
    await h.win.reset();
    h.win.setRange(0, 5);
    await tick();
    h.setList([3, 1, 2]); // 4 and 5 no longer match
    await h.win.refresh();
    expect(h.win.total.value).toBe(3);
    expect(h.win.idAt(0)).toBe(3);
    expect(h.win.getById(4)).toBeUndefined();
    expect(h.win.getById(3)).toBeDefined();
  });

  it("keeps the wanted range, so the same rows reload without the view asking again", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(300, 340);
    await tick();
    const before = h.calls.length;
    await h.win.refresh();
    await tick();
    expect(h.calls.length).toBe(before + 1);
  });

  it("ignores an older refresh that finishes after a newer one", async () => {
    const h = harness(10);
    await h.win.reset();
    const a = h.win.refresh();
    h.setList([10, 9]);
    const b = h.win.refresh();
    await Promise.all([a, b]);
    expect(h.win.ids.value).toEqual([10, 9]);
  });
});

describe("reset", () => {
  it("forgets the old query's rows and signals views to scroll to the top", async () => {
    const h = harness(300);
    await h.win.reset();
    const before = h.win.resetCount.value;
    h.win.setRange(0, 50);
    await tick();
    expect(h.win.getById(1)).toBeDefined();

    h.setList([7, 8]);
    await h.win.reset();
    expect(h.win.resetCount.value).toBe(before + 1);
    expect(h.win.getById(1)).toBeUndefined();
    expect(h.win.ids.value).toEqual([7, 8]);
  });

  it("drops a chunk that was still in flight for the old query", async () => {
    const h = harness(300);
    await h.win.reset();
    h.hold();
    h.win.setRange(0, 50);
    h.setList([500, 501]);
    await h.win.reset();
    await h.release();
    expect(h.win.getById(1)).toBeUndefined();
  });

  it("reports a failed id fetch and empties the list", async () => {
    const win = createBookWindow({
      fetchIds: async () => {
        throw new Error("server down");
      },
      fetchBooks: async () => [],
    });
    await win.reset();
    expect(win.error.value).toBe("server down");
    expect(win.total.value).toBe(0);
  });
});

describe("selection over rows that are not loaded", () => {
  it("returns every id between two rows for a shift-range, either direction", async () => {
    const h = harness(50000);
    await h.win.reset();
    const range = h.win.idsBetween(10, 40000);
    expect(range).toHaveLength(39991);
    expect(range[0]).toBe(11);
    expect(h.win.idsBetween(40000, 10)).toEqual(range);
    expect(h.calls).toHaveLength(0); // nothing was fetched to answer that
  });

  it("clamps a range to the list", async () => {
    const h = harness(10);
    await h.win.reset();
    expect(h.win.idsBetween(-5, 99)).toHaveLength(10);
  });

  it("select-all is the whole id list, with no summaries fetched", async () => {
    const h = harness(9767);
    await h.win.reset();
    expect(new Set(h.win.ids.value).size).toBe(9767);
    expect(h.calls).toHaveLength(0);
  });

  it("ensureLoaded fetches only the unloaded ids, in chunks", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(0, 10);
    await tick();
    h.calls.length = 0;
    const books = await h.win.ensureLoaded([1, 2, ...Array.from({ length: 150 }, (_, i) => 500 + i)]);
    expect(books).toHaveLength(152);
    expect(h.calls.map((c) => c.length)).toEqual([100, 50]);
    expect(h.calls[0]).not.toContain(1);
  });
});

describe("edits", () => {
  it("patches a cached row and returns the previous one for rollback", async () => {
    const h = harness(10);
    await h.win.reset();
    h.win.setRange(0, 10);
    await tick();
    const before = h.win.patch(3, { title: "New" });
    expect(h.win.getById(3)?.title).toBe("New");
    expect(before?.title).toBe("Book 3");
    h.win.put(before!);
    expect(h.win.getById(3)?.title).toBe("Book 3");
    expect(h.win.patch(999, { title: "x" })).toBeUndefined();
  });

  it("invalidate refetches just the chunk of the given ids", async () => {
    const h = harness(1000);
    await h.win.reset();
    h.win.setRange(0, 250);
    await tick();
    await tick();
    const before = h.calls.length;
    h.win.invalidate([150]);
    await tick();
    expect(h.calls.length - before).toBe(1);
    expect(h.calls[h.calls.length - 1][0]).toBe(101);
  });
});
