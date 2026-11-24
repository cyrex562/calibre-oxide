import { describe, expect, it } from "vitest";

import {
  confirmDeleteForever,
  confirmEmpty,
  describeKind,
  describeRestored,
  describeTrash,
  entryId,
  formatSize,
  restoreRequests,
  sortEntries,
  timeLeft,
  type TrashEntry,
} from "./trash";

function entry(over: Partial<TrashEntry> = {}): TrashEntry {
  return {
    kind: "book",
    key: "7",
    book_id: 7,
    title: "Dune",
    authors: "Frank Herbert",
    formats: ["EPUB", "PDF"],
    size: 3 * 1024 * 1024,
    deleted_at: 1_700_000_000,
    expires_at: 1_700_000_000 + 14 * 86400,
    original_path: "Frank Herbert/Dune (7)",
    problem: null,
    ...over,
  };
}

describe("formatSize", () => {
  it("reads like a file manager", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(1023)).toBe("1023 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(3 * 1024 * 1024)).toBe("3.0 MB");
    expect(formatSize(250 * 1024 * 1024)).toBe("250 MB");
    expect(formatSize(5 * 1024 ** 4)).toBe("5.0 TB");
  });

  it("does not print nonsense for a bad number", () => {
    expect(formatSize(-1)).toBe("—");
    expect(formatSize(NaN)).toBe("—");
  });
});

describe("timeLeft", () => {
  const e = entry();
  it("counts days, then hours", () => {
    expect(timeLeft(e, e.deleted_at)).toBe("14 days left");
    expect(timeLeft(e, e.expires_at - 86400)).toBe("1 day left");
    expect(timeLeft(e, e.expires_at - 5 * 3600)).toBe("5 hours left");
    expect(timeLeft(e, e.expires_at - 3600)).toBe("1 hour left");
    expect(timeLeft(e, e.expires_at - 60)).toBe("less than an hour left");
  });

  it("says so when it is already past due, since cleanup runs hourly rather than at the moment", () => {
    expect(timeLeft(e, e.expires_at + 5)).toBe("expires at the next cleanup");
  });
});

describe("describing entries", () => {
  it("names a book by its formats and a format entry by the format", () => {
    expect(describeKind(entry())).toBe("Book (EPUB, PDF)");
    expect(describeKind(entry({ formats: [] }))).toBe("Book");
    expect(describeKind(entry({ kind: "format", formats: ["PDF"] }))).toBe("Format PDF");
    expect(describeKind(entry({ kind: "format", formats: ["PDF", "MOBI"] }))).toBe("Formats PDF, MOBI");
  });

  it("gives a book and a format entry of the same book different row ids", () => {
    expect(entryId(entry())).not.toBe(entryId(entry({ kind: "format" })));
  });

  it("sorts the newest deletion first and does not mutate its input", () => {
    const older = entry({ key: "1", deleted_at: 100 });
    const newer = entry({ key: "2", deleted_at: 200 });
    const input = [older, newer];
    expect(sortEntries(input).map((e) => e.key)).toEqual(["2", "1"]);
    expect(input.map((e) => e.key)).toEqual(["1", "2"]);
  });
});

describe("restoring", () => {
  it("asks for a book by key alone", () => {
    expect(restoreRequests(entry())).toEqual([{ kind: "book", key: "7" }]);
  });

  it("asks for each format of a format entry, because the server restores one at a time", () => {
    expect(restoreRequests(entry({ kind: "format", formats: ["PDF", "MOBI"] }))).toEqual([
      { kind: "format", key: "7", format: "PDF" },
      { kind: "format", key: "7", format: "MOBI" },
    ]);
  });

  it("says what was restored and mentions a file that had to be renamed", () => {
    expect(describeRestored([{ title: "Dune" }])).toBe("Restored “Dune”.");
    expect(describeRestored([{ format: "PDF" }])).toBe("Restored the PDF.");
    expect(describeRestored([{ title: "A" }, { title: "B" }])).toBe("Restored 2 items.");
    expect(describeRestored([{ title: "Dune", renamed: [["d.pdf", "d (restored).pdf"]] }])).toContain("restored as “d (restored).pdf”");
  });
});

describe("confirmations", () => {
  it("names the one thing being deleted for good, with its size, and says it cannot be undone", () => {
    const text = confirmDeleteForever([entry()]);
    expect(text).toContain("“Dune”");
    expect(text).toContain("3.0 MB");
    expect(text).toContain("cannot be undone");
  });

  it("counts when there are several", () => {
    expect(confirmDeleteForever([entry(), entry({ key: "8", size: 1024 * 1024 })])).toContain("these 2 items (4.0 MB)");
  });

  it("says how much emptying will destroy", () => {
    expect(confirmEmpty({ count: 1, total_size: 1024 })).toBe("Empty the trash? 1 item (1.0 KB) will be permanently deleted. This cannot be undone.");
    expect(confirmEmpty({ count: 12, total_size: 0 })).toContain("12 items");
  });
});

describe("describeTrash", () => {
  it("says it is empty, or how long things last", () => {
    expect(describeTrash({ count: 0, total_size: 0, expire_after_seconds: 14 * 86400, expired: [], entries: [] })).toBe("The trash is empty.");
    expect(describeTrash({ count: 2, total_size: 2048, expire_after_seconds: 14 * 86400, expired: [], entries: [] })).toBe("2 items, 2.0 KB. Items are deleted for good after 14 days.");
    expect(describeTrash({ count: 1, total_size: 10, expire_after_seconds: 86400, expired: [], entries: [] })).toContain("after 1 day.");
  });

  it("explains the longer keeping of books an undoable run removed, so no row contradicts the header", () => {
    const kept = entry({ expires_at: 1_700_000_000 + 180 * 86400 });
    const text = describeTrash({ count: 2, total_size: 2048, expire_after_seconds: 14 * 86400, expired: [], entries: [entry(), kept] });
    expect(text).toBe("2 items, 2.0 KB. Items are deleted for good after 14 days (180 days for books a duplicate run removed, so that run can still be undone).");
    expect(describeTrash({ count: 1, total_size: 2048, expire_after_seconds: 14 * 86400, expired: [], entries: [entry()] })).toBe("1 item, 2.0 KB. Items are deleted for good after 14 days.");
  });
});
