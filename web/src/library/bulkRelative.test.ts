import { describe, expect, it } from "vitest";

import { emptySpec } from "./useBulkEdit";
import { RELATIVE_CHUNK, RelativeUnsupportedError, relativeTagOp, runRelativeEdit, type RelativeOp, type RelativeResponse } from "./bulkRelative";

const op: RelativeOp = { field: "tags", mode: "add", values: ["x"] };
const ids = (n: number) => Array.from({ length: n }, (_, i) => i + 1);

describe("relativeTagOp", () => {
  it("is an add or remove of tags and nothing else", () => {
    const spec = emptySpec();
    spec.tags = { mode: "add", value: " sci-fi , history,, " };
    expect(relativeTagOp(spec)).toEqual({ field: "tags", mode: "add", values: ["sci-fi", "history"] });
    spec.tags = { mode: "remove", value: "old" };
    expect(relativeTagOp(spec)).toEqual({ field: "tags", mode: "remove", values: ["old"] });
  });

  it("is not one when tags are replaced, empty, or anything else is edited too", () => {
    const base = () => {
      const s = emptySpec();
      s.tags = { mode: "add", value: "x" };
      return s;
    };
    expect(relativeTagOp(emptySpec())).toBeNull();
    const replace = base();
    replace.tags = { mode: "replace", value: "x" };
    expect(relativeTagOp(replace)).toBeNull();
    const publisher = base();
    publisher.publisher = "Acme";
    expect(relativeTagOp(publisher)).toBeNull();
    const authors = base();
    authors.authors = { mode: "add", value: "Someone" };
    expect(relativeTagOp(authors)).toBeNull();
    const rating = base();
    rating.rating = 4;
    expect(relativeTagOp(rating)).toBeNull();
    const custom = base();
    custom.custom = { flag: "yes" };
    expect(relativeTagOp(custom)).toBeNull();
    const sr = base();
    sr.searchReplace = { field: "title", find: "a", replace: "b", useRegex: false };
    expect(relativeTagOp(sr)).toBeNull();
  });
});

describe("runRelativeEdit", () => {
  const ok = async (slice: number[]): Promise<RelativeResponse> => ({ updated: slice, failed: [] });

  it("sends the books in slices and reports progress, without reading any book first", async () => {
    const sizes: number[] = [];
    const progress: [number, number][] = [];
    const result = await runRelativeEdit(ids(RELATIVE_CHUNK * 2 + 5), op, {
      send: async (slice) => {
        sizes.push(slice.length);
        return ok(slice);
      },
      onProgress: (d, t) => progress.push([d, t]),
    });
    expect(sizes).toEqual([RELATIVE_CHUNK, RELATIVE_CHUNK, 5]);
    expect(result?.updated).toBe(RELATIVE_CHUNK * 2 + 5);
    expect(progress[progress.length - 1]).toEqual([RELATIVE_CHUNK * 2 + 5, RELATIVE_CHUNK * 2 + 5]);
  });

  it("counts books that already had the result, and keeps per-book failures", async () => {
    const result = await runRelativeEdit(ids(4), op, {
      send: async () => ({ updated: [1], unchanged: [2, 3], failed: [{ book_id: 4, error: "gone" }] }),
    });
    expect(result).toEqual({ updated: 1, unchanged: 2, errors: ["Book 4: gone"], notStarted: 0 });
  });

  it("returns null, having sent nothing else, when the server does not know add/remove", async () => {
    let calls = 0;
    const result = await runRelativeEdit(ids(10), op, {
      send: async () => {
        calls += 1;
        throw new RelativeUnsupportedError();
      },
    });
    expect(result).toBeNull();
    expect(calls).toBe(1);
  });

  it("stops starting slices once cancelled", async () => {
    let sent = 0;
    const result = await runRelativeEdit(ids(10), op, {
      chunkSize: 4,
      send: async (slice) => {
        sent += 1;
        return ok(slice);
      },
      cancelled: () => sent >= 1,
    });
    expect(result?.updated).toBe(4);
    expect(result?.notStarted).toBe(6);
  });

  it("marks every book of a slice that failed outright", async () => {
    const result = await runRelativeEdit(ids(3), op, {
      send: async () => {
        throw new Error("connection dropped");
      },
    });
    expect(result?.errors).toEqual(["Book 1: connection dropped", "Book 2: connection dropped", "Book 3: connection dropped"]);
  });
});
