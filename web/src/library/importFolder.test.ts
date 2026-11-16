import { describe, expect, it } from "vitest";

import { DEFAULT_IMPORT, DUPLICATE_POLICIES, IMPORT_MODES, describeImport, importBody, importProblems, validateSource, type ImportReport } from "./importFolder";

function report(over: Partial<ImportReport> = {}): ImportReport {
  return {
    source: "/books",
    files_seen: 0,
    books_found: 0,
    books_added: 0,
    formats_added_to_existing: 0,
    skipped_already_imported: 0,
    skipped_duplicate_content: 0,
    skipped_duplicate_title: 0,
    added: [],
    duplicates: [],
    skipped_same_format: [],
    failed: [],
    walk_errors: [],
    unmatched_without_author: 0,
    cancelled: false,
    conclusive: true,
    ...over,
  };
}

describe("the choices", () => {
  it("defaults to calibre's grouping by name, skipping identical files, descending into subfolders", () => {
    expect(DEFAULT_IMPORT).toEqual({ mode: "multiple_books_per_folder", policy: "skip_exact_content", recursive: true });
  });

  it("offers every mode and policy the server knows, each explained", () => {
    expect(IMPORT_MODES.map((m) => m.value)).toEqual(["multiple_books_per_folder", "one_book_per_folder"]);
    expect(DUPLICATE_POLICIES.map((p) => p.value)).toEqual(["skip_exact_content", "skip_title_author", "add_formats_to_existing", "add_anyway"]);
    for (const c of [...IMPORT_MODES, ...DUPLICATE_POLICIES]) {
      expect(c.label.length).toBeGreaterThan(3);
      expect(c.help.length).toBeGreaterThan(20);
    }
  });

  it("makes the default policy the first one offered", () => {
    expect(DUPLICATE_POLICIES[0].value).toBe(DEFAULT_IMPORT.policy);
    expect(IMPORT_MODES[0].value).toBe(DEFAULT_IMPORT.mode);
  });
});

describe("validateSource", () => {
  it("accepts absolute paths on either platform, including a share", () => {
    for (const ok of ["/home/me/books", "C:\\Books", "d:/books", "\\\\nas\\share\\books"]) expect(validateSource(ok), ok).toBeNull();
  });

  it("asks for a folder when there is none, and for a full path when it is relative", () => {
    expect(validateSource("   ")).toMatch(/Choose the folder/);
    expect(validateSource("books/inbox")).toMatch(/full path/);
    expect(validateSource("../books")).toMatch(/full path/);
  });
});

describe("importBody", () => {
  it("trims the path and sends exactly the four choices", () => {
    expect(importBody({ source: "  /books  ", mode: "one_book_per_folder", policy: "add_anyway", recursive: false })).toEqual({
      source: "/books",
      mode: "one_book_per_folder",
      policy: "add_anyway",
      recursive: false,
    });
  });
});

describe("describeImport", () => {
  it("says nothing was added rather than 'Added 0 books'", () => {
    expect(describeImport(report())).toEqual(["Nothing new was added."]);
  });

  it("counts books, and formats added to existing books", () => {
    expect(describeImport(report({ books_added: 1 }))[0]).toBe("Added 1 book.");
    expect(describeImport(report({ books_added: 2500, formats_added_to_existing: 3 }))[0]).toBe("Added 2,500 books and 3 formats to existing books.");
    expect(describeImport(report({ formats_added_to_existing: 1 }))[0]).toBe("Added 0 books and 1 format to existing books.");
  });

  it("explains each kind of skip separately, because they mean different things", () => {
    const lines = describeImport(report({ books_added: 1, skipped_already_imported: 4, skipped_duplicate_content: 2, skipped_duplicate_title: 1, skipped_same_format: ["/b/two.pdf"], unmatched_without_author: 5 }));
    expect(lines).toContain("4 files had been imported before.");
    expect(lines).toContain("2 files skipped: identical to a file already in the library.");
    expect(lines).toContain("1 file skipped: the same title and author as a book already in the library.");
    expect(lines).toContain("1 file skipped: a second file of the same format in a one-book folder.");
    expect(lines).toContain("5 books had no author, so could not be matched by title.");
  });

  it("says when it was stopped and how to continue", () => {
    const lines = describeImport(report({ cancelled: true, conclusive: false }));
    expect(lines.join(" ")).toMatch(/Stopped early.*again.*without being read/);
  });

  it("never hides failures or unreadable folders", () => {
    const lines = describeImport(report({ failed: [{ path: "/b/x.pdf", error: "locked" }], walk_errors: [{ path: "/b/private", error: "denied" }, { path: "/b/other", error: "gone" }] }));
    expect(lines).toContain("1 file could not be imported.");
    expect(lines).toContain("2 folders could not be read, so the import is incomplete.");
  });
});

describe("importProblems", () => {
  it("lists walk errors then file failures with their reasons", () => {
    expect(importProblems(report({ failed: [{ path: "/b/x.pdf", error: "locked" }], walk_errors: [{ path: "/b/private", error: "denied" }] }))).toEqual(["/b/private: denied", "/b/x.pdf: locked"]);
  });
});
