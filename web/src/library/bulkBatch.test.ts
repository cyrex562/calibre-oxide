import { describe, expect, it } from "vitest";

import { BATCH_CHUNK, BatchUnsupportedError, groupByChanges, runBulkEdit, type BatchGroup, type BatchResponse, type BulkItem } from "./bulkBatch";

const items = (n: number, changes: (i: number) => Record<string, unknown> = () => ({ publisher: "Acme" })): BulkItem[] => Array.from({ length: n }, (_, i) => ({ id: i + 1, changes: changes(i) }));

const everyoneUpdated = async (groups: BatchGroup[]): Promise<BatchResponse> => ({ updated: groups.flatMap((g) => g.book_ids), failed: [] });

describe("groupByChanges", () => {
  it("puts books with identical changes in one group, in first-seen order", () => {
    const groups = groupByChanges([
      { id: 1, changes: { tags: ["a"] } },
      { id: 2, changes: { tags: ["a", "b"] } },
      { id: 3, changes: { tags: ["a"] } },
    ]);
    expect(groups).toEqual([
      { book_ids: [1, 3], changes: { tags: ["a"] } },
      { book_ids: [2], changes: { tags: ["a", "b"] } },
    ]);
  });
});

describe("runBulkEdit previous values", () => {
  it("collects what the server says each written book held before", async () => {
    const result = await runBulkEdit(items(3), {
      sendBatch: async (groups) => ({ updated: groups.flatMap((g) => g.book_ids), failed: [], previous: { "1": { publisher: "A" }, "2": { publisher: null }, "3": { publisher: "C" } } }),
      sendOne: async () => ({}),
    });
    expect([...result.previous.entries()]).toEqual([
      [1, { publisher: "A" }],
      [2, { publisher: null }],
      [3, { publisher: "C" }],
    ]);
  });

  it("has none from a server that does not say", async () => {
    expect((await runBulkEdit(items(3), { sendBatch: everyoneUpdated, sendOne: async () => ({}) })).previous.size).toBe(0);
  });
});

describe("runBulkEdit", () => {
  it("sends about 200 books per request and reports progress per chunk", async () => {
    const sizes: number[] = [];
    const progress: [number, number][] = [];
    const result = await runBulkEdit(items(450), {
      sendBatch: async (groups) => {
        sizes.push(groups.reduce((n, g) => n + g.book_ids.length, 0));
        return everyoneUpdated(groups);
      },
      sendOne: async () => {
        throw new Error("the per-book route is not used when the batch route exists");
      },
      onProgress: (d, t) => progress.push([d, t]),
    });
    expect(BATCH_CHUNK).toBe(200);
    expect(sizes).toEqual([200, 200, 50]);
    expect(progress).toEqual([[200, 450], [400, 450], [450, 450]]);
    expect(result).toEqual({ updated: 450, errors: [], notStarted: 0, usedFallback: false, previous: new Map() });
  });

  it("sends one group per distinct change set within a chunk", async () => {
    const seen: BatchGroup[][] = [];
    await runBulkEdit(items(6, (i) => ({ tags: i % 2 ? ["x"] : ["y"] })), {
      sendBatch: async (groups) => {
        seen.push(groups);
        return everyoneUpdated(groups);
      },
      sendOne: async () => undefined,
    });
    expect(seen).toHaveLength(1);
    expect(seen[0].map((g) => g.book_ids)).toEqual([[1, 3, 5], [2, 4, 6]]);
  });

  it("stops after the chunk that is running when cancelled", async () => {
    let cancelled = false;
    let requests = 0;
    const result = await runBulkEdit(items(700), {
      sendBatch: async (groups) => {
        requests += 1;
        cancelled = true; // the user pressed Cancel during the first request
        return everyoneUpdated(groups);
      },
      sendOne: async () => undefined,
      cancelled: () => cancelled,
    });
    expect(requests).toBe(1);
    expect(result.updated).toBe(200);
    expect(result.notStarted).toBe(500);
  });

  it("reports the books the server could not update and carries on", async () => {
    const result = await runBulkEdit(items(3), {
      sendBatch: async () => ({ updated: [1, 3], failed: [{ book_id: 2, error: "Book 2 not found" }] }),
      sendOne: async () => undefined,
    });
    expect(result.updated).toBe(2);
    expect(result.errors).toEqual(["Book 2: Book 2 not found"]);
  });

  it("marks every book of a request that failed outright, then sends the next chunk", async () => {
    let calls = 0;
    const result = await runBulkEdit(items(250), {
      sendBatch: async (groups) => {
        calls += 1;
        if (calls === 1) throw new Error("network down");
        return everyoneUpdated(groups);
      },
      sendOne: async () => undefined,
    });
    expect(result.updated).toBe(50);
    expect(result.errors).toHaveLength(200);
    expect(result.errors[0]).toBe("Book 1: network down");
  });

  it("falls back to one request per book when the server has no batch route", async () => {
    const sent: number[] = [];
    const progress: number[] = [];
    const result = await runBulkEdit(items(10), {
      sendBatch: async () => {
        throw new BatchUnsupportedError();
      },
      sendOne: async (id) => {
        sent.push(id);
      },
      onProgress: (d) => progress.push(d),
    });
    expect(sent.sort((a, b) => a - b)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(result).toEqual({ updated: 10, errors: [], notStarted: 0, usedFallback: true, previous: new Map() });
    expect(progress[progress.length - 1]).toBe(10);
  });

  it("falls back for the books after the chunks that did go through", async () => {
    let calls = 0;
    const sent: number[] = [];
    const result = await runBulkEdit(items(250), {
      sendBatch: async (groups) => {
        calls += 1;
        if (calls === 2) throw new BatchUnsupportedError();
        return everyoneUpdated(groups);
      },
      sendOne: async (id) => {
        sent.push(id);
      },
    });
    expect(sent).toHaveLength(50);
    expect(Math.min(...sent)).toBe(201);
    expect(result.updated).toBe(250);
    expect(result.usedFallback).toBe(true);
  });

  it("does nothing for an empty list", async () => {
    const result = await runBulkEdit([], { sendBatch: everyoneUpdated, sendOne: async () => undefined });
    expect(result).toEqual({ updated: 0, errors: [], notStarted: 0, usedFallback: false, previous: new Map() });
  });
});
