import { describe, expect, it } from "vitest";

import { absorbProblems, describeAbsorb, describeAbsorbable, type AbsorbReport, type AbsorbableLibrary } from "./absorb";

function report(over: Partial<AbsorbReport> = {}): AbsorbReport {
  return {
    folder: "NAS/Calibre",
    source_books: 4,
    absorbed: 4,
    already_absorbed: 0,
    formats: 5,
    covers: 3,
    annotations: 0,
    custom_values: 20,
    failed: [],
    failed_count: 0,
    missing_files: [],
    missing_count: 0,
    uuid_remapped: [],
    custom_columns_created: [],
    custom_column_conflicts: [],
    warnings: [],
    cancelled: false,
    renamed: true,
    not_renamed_because: null,
    not_carried: [],
    ...over,
  };
}

const lib = (over: Partial<AbsorbableLibrary> = {}): AbsorbableLibrary => ({ path: "NAS/Calibre", state: "available", books: 1200, already_absorbed: 0, problem: null, ...over });

describe("describeAbsorbable", () => {
  it("says how many books, and where a partly done absorb stands", () => {
    expect(describeAbsorbable(lib())).toBe("1,200 books");
    expect(describeAbsorbable(lib({ books: 1, already_absorbed: 0 }))).toBe("1 book");
    expect(describeAbsorbable(lib({ already_absorbed: 300 }))).toBe("1,200 books, 300 already in — partly absorbed");
    expect(describeAbsorbable(lib({ state: "absorbed" }))).toBe("1,200 books in this library");
    expect(describeAbsorbable(lib({ problem: "not a calibre library database" }))).toContain("Could not be read");
  });
});

describe("describeAbsorb", () => {
  it("reports what came in and that the database was renamed", () => {
    const lines = describeAbsorb(report());
    expect(lines[0]).toBe("Brought in 4 books (5 files, 3 covers, 20 custom values).");
    expect(lines.join(" ")).toContain("metadata.db.absorbed-by-calibre-oxide");
  });

  it("says when nothing was new, and when it was stopped", () => {
    expect(describeAbsorb(report({ absorbed: 0, already_absorbed: 4, renamed: false }))[0]).toBe("Nothing new: every book was in already.");
    const stopped = describeAbsorb(report({ absorbed: 2, cancelled: true, renamed: false, not_renamed_because: "the job was cancelled; run it again to carry on" }));
    expect(stopped.some((l) => l.startsWith("Stopped early"))).toBe(true);
    expect(stopped.some((l) => l.startsWith("The job was cancelled"))).toBe(true);
  });

  it("lists problems the user may need to act on", () => {
    const r = report({ failed_count: 1, failed: [{ source_id: 9, title: "Odd Book", error: "boom" }], missing_count: 1, missing_files: ["a/b.epub"], custom_column_conflicts: ["#pages: text here, int in the nested library"] });
    expect(absorbProblems(r)).toEqual(["Custom column #pages: text here, int in the nested library; its values were not carried.", "Odd Book: boom", "Missing: a/b.epub"]);
    expect(describeAbsorb(r).join(" ")).toContain("1 book could not be brought in");
  });
});
