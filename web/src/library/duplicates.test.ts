import { describe, expect, it } from "vitest";
import { ALL_REASONS, analyzeLabelFor, pageMatchNoteFor, DEFAULT_RESOLVE_TIERS, MIN_OPTIONS, confidencePercent, countByReason, evidenceLines, filterGroups, fileSummary, fingerprintAction, formatMtime, isImageOnlyScan, isMetadataOnly, isSuggested, needsReview, pdfLine, primaryPath, reasonBadge, reasonLabel, deleteCopyWarnings } from "./duplicates";
import type { DuplicateGroup, DuplicateReason } from "./api";

const group = (reason: DuplicateReason, confidence = 0.9): DuplicateGroup => ({ books: [], reason, confidence });

const groups = [group("title_author_fuzzy", 0.65), group("content", 1), group("title_author_exact"), group("identifier", 0.97), group("title_author_fuzzy", 0.6)];

describe("filterGroups", () => {
  it("shows everything at fuzzy", () => {
    expect(filterGroups(groups, "fuzzy")).toHaveLength(5);
  });
  it("hides fuzzy groups at exact", () => {
    expect(filterGroups(groups, "exact").map((g) => g.reason)).toEqual(["content", "title_author_exact", "identifier"]);
  });
  it("keeps only identifier and content at identifier", () => {
    expect(filterGroups(groups, "identifier").map((g) => g.reason)).toEqual(["content", "identifier"]);
  });
  it("keeps only identical files at content", () => {
    expect(filterGroups(groups, "content").map((g) => g.reason)).toEqual(["content"]);
  });
  it("keeps order and does not mutate", () => {
    const copy = [...groups];
    filterGroups(groups, "exact");
    expect(groups).toEqual(copy);
  });
  it("drops a reason it does not know rather than showing it as certain", () => {
    expect(filterGroups([{ reason: "something_new" as DuplicateReason }], "exact")).toEqual([]);
  });
});

describe("countByReason", () => {
  it("counts what each filter would show", () => {
    expect(countByReason(groups)).toEqual({ fuzzy: 5, pdf: 3, exact: 3, identifier: 2, content: 1 });
  });
  it("is zero for an empty scan", () => {
    expect(countByReason([])).toEqual({ fuzzy: 0, pdf: 0, exact: 0, identifier: 0, content: 0 });
  });
});

describe("badges", () => {
  it("has a distinct badge per reason", () => {
    const badges = (["content", "identifier", "title_author_exact", "title_author_fuzzy"] as const).map((r) => reasonBadge(r));
    expect(new Set(badges.map((b) => b.text)).size).toBe(4);
    expect(new Set(badges.map((b) => b.tone)).size).toBe(4);
  });
  it("marks only a fuzzy group as needing review", () => {
    expect(reasonBadge("title_author_fuzzy").tone).toBe("review");
    expect(needsReview(group("title_author_fuzzy"))).toBe(true);
    for (const r of ["content", "identifier", "title_author_exact"] as const) expect(needsReview(group(r))).toBe(false);
  });
  it("labels reasons in words", () => {
    expect(reasonLabel("content")).toBe("identical files");
    expect(reasonLabel("title_author_fuzzy")).toBe("probably the same book");
  });
  it("shows confidence as a whole percent", () => {
    expect(confidencePercent(0.9)).toBe("90%");
    expect(confidencePercent(0.655)).toBe("66%");
    expect(confidencePercent(1)).toBe("100%");
  });
});

describe("evidenceLines", () => {
  it("summarises each piece of evidence", () => {
    const lines = evidenceLines({ evidence: [{ books: [1, 2], reason: "identifier", confidence: 0.97, detail: "same isbn:9780261102217" }] });
    expect(lines).toEqual(["same ISBN or identifier (97%): same isbn:9780261102217"]);
  });
  it("copes with no evidence", () => {
    expect(evidenceLines({})).toEqual([]);
  });
});

describe("MIN_OPTIONS", () => {
  it("lists the filters, weakest first, with the default first and pdf between fuzzy and exact", () => {
    expect(MIN_OPTIONS.map((o) => o.value)).toEqual(["fuzzy", "pdf", "exact", "identifier", "content"]);
  });
});

describe("PDF reasons", () => {
  const all = [group("content", 1), group("title_author_exact"), group("pdf_resave", 0.85), group("pdf_ocr_variant", 0.8), group("pdf_text_match", 0.6), group("title_author_fuzzy", 0.6)];

  it("names the new reasons for the user", () => {
    expect(reasonBadge("pdf_ocr_variant").text).toBe("Scan vs OCR");
    expect(reasonBadge("pdf_resave").text).toBe("Re-saved PDF");
    expect(reasonLabel("pdf_ocr_variant")).toContain("OCR");
  });

  it("puts the pdf level between exact and fuzzy", () => {
    expect(filterGroups(all, "pdf").map((g) => g.reason)).toEqual(["content", "title_author_exact", "pdf_resave", "pdf_ocr_variant", "pdf_text_match"]);
    expect(filterGroups(all, "exact").map((g) => g.reason)).toEqual(["content", "title_author_exact"]);
    expect(filterGroups(all, "fuzzy")).toHaveLength(6);
  });

  it("asks for review only when the match is on text alone", () => {
    expect(needsReview(group("pdf_text_match"))).toBe(true);
    expect(needsReview(group("pdf_ocr_variant"))).toBe(false);
    expect(needsReview(group("pdf_resave"))).toBe(false);
  });
});

describe("pdfLine", () => {
  it("says pages and whether there is a text layer, so a scan can be told from its OCR", () => {
    expect(pdfLine({ pdf: { pages: 312, has_text: false, size: 120 * 1024 * 1024 } })).toBe("312 pages · image only · 120.0 MB");
    expect(pdfLine({ pdf: { pages: 1, has_text: true, size: 2048 } })).toBe("1 page · text layer · 2 KB");
  });
  it("is empty for a book that is not a fingerprinted PDF", () => {
    expect(pdfLine({})).toBe("");
    expect(pdfLine({ pdf: null })).toBe("");
  });
});

describe("fingerprintAction", () => {
  it("offers the job with the count of PDFs not yet analyzed", () => {
    expect(fingerprintAction({ fingerprinted: 10, failed: 0, pending: 1234, unhashed: 0 })).toEqual({ label: "Analyze library (1,234 not yet analyzed)", note: null });
  });
  it("says what to do about PDFs that have no hash yet", () => {
    expect(fingerprintAction({ fingerprinted: 0, failed: 0, pending: 5, unhashed: 7 })?.note).toContain("Analyze library");
  });
  it("offers nothing when everything is analyzed, or the server did not say", () => {
    expect(fingerprintAction({ fingerprinted: 10, failed: 2, pending: 0, unhashed: 0 })).toBeNull();
    expect(fingerprintAction(undefined)).toBeNull();
  });
});

describe("isSuggested", () => {
  it("marks the server's suggested copy", () => {
    expect(isSuggested({ suggested_dest: 4 }, { book_id: 4 })).toBe(true);
    expect(isSuggested({ suggested_dest: 4 }, { book_id: 5 })).toBe(false);
    expect(isSuggested({ suggested_dest: null }, { book_id: 5 })).toBe(false);
  });
});

describe("the compare card's lines (tranche 12)", () => {
  const book = { book_id: 1, title: "T", authors: [], files: [{ format: "EPUB", path: "A/T.epub", size: 1536, mtime: 0 }, { format: "PDF", path: "A/T.pdf", size: 3 * 1024 * 1024, mtime: 0 }], path: "A" };
  it("lists each format with its size, and shows the first file's path", () => {
    expect(fileSummary(book)).toBe("EPUB 2 KB · PDF 3.0 MB");
    expect(primaryPath(book)).toBe("A/T.epub");
    expect(primaryPath({ path: "Folder" })).toBe("Folder");
    expect(primaryPath({})).toBe("");
  });
  it("formats a modified time, or nothing when unknown", () => {
    expect(formatMtime(null)).toBe("");
    expect(formatMtime(0)).toBe("");
    expect(formatMtime(1_700_000_000)).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  });
  it("calls a PDF with pages and no text layer an image-only scan", () => {
    expect(isImageOnlyScan({ pdf: { pages: 10, has_text: false, scanned: true, size: 1 } })).toBe(true);
    expect(isImageOnlyScan({ pdf: { pages: 10, has_text: true, scanned: true, size: 1 } })).toBe(false);
    expect(isImageOnlyScan({ pdf: null })).toBe(false);
  });
  it("knows which tiers are metadata only", () => {
    expect(["content", "pdf_resave", "pdf_ocr_variant", "pdf_text_match"].some((r) => isMetadataOnly(r as DuplicateReason))).toBe(false);
    expect(["identifier", "title_author_exact", "title_author_fuzzy"].every((r) => isMetadataOnly(r as DuplicateReason))).toBe(true);
  });
});

describe("the user's own groups", () => {
  it("are badged as yours, certain, and labelled", () => {
    expect(reasonBadge("manual")).toEqual({ text: "Marked by you", tone: "certain" });
    expect(reasonLabel("manual")).toBe("groups I made by hand");
    expect(needsReview({ reason: "manual" })).toBe(false);
  });

  it("are the strongest kind, so every minimum level still shows them", () => {
    expect(ALL_REASONS[0]).toBe("manual");
    const mixed = [group("manual", 1), group("content", 1), group("pdf_resave", 0.97)];
    expect(filterGroups(mixed, "content").map((g) => g.reason)).toEqual(["manual", "content"]);
    expect(filterGroups(mixed, "pdf")).toHaveLength(3);
  });

  it("are metadata-only for a bulk resolve, so never ticked by default", () => {
    expect(isMetadataOnly("manual")).toBe(true);
    expect(DEFAULT_RESOLVE_TIERS).not.toContain("manual");
  });

  it("show a graded PDF confidence as a whole percent", () => {
    expect(confidencePercent(0.9132)).toBe("91%");
    expect(confidencePercent(0.5)).toBe("50%");
    expect(confidencePercent(0.97)).toBe("97%");
  });
});

describe("deleteCopyWarnings", () => {
  const group = (ids: number[]) => ({ books: ids.map((book_id) => ({ book_id, title: `T${book_id}`, authors: [] })) });
  it("warns before deleting the copy suggested to keep", () => {
    expect(deleteCopyWarnings(group([1, 2]), { book_id: 1 }, 1)).toEqual(["This is the copy suggested to keep."]);
  });
  it("warns before deleting the last copy left in a group", () => {
    const w = deleteCopyWarnings(group([5]), { book_id: 5 }, 5);
    expect(w).toHaveLength(1);
    expect(w[0]).toContain("last copy");
  });
  it("says nothing for an ordinary extra copy", () => {
    expect(deleteCopyWarnings(group([1, 2]), { book_id: 2 }, 1)).toEqual([]);
    expect(deleteCopyWarnings(group([1, 2]), { book_id: 2 }, null)).toEqual([]);
  });
});

describe("analyzeLabelFor", () => {
  it("names how many PDFs are still to analyze, counting the ones not yet hashed", () => {
    expect(analyzeLabelFor({ fingerprinted: 10, failed: 0, pending: 60, unhashed: 0 })).toBe("Analyze library (60 PDFs not yet analyzed)");
    expect(analyzeLabelFor({ fingerprinted: 0, failed: 0, pending: 0, unhashed: 1 })).toBe("Analyze library (1 PDF not yet analyzed)");
  });
  it("is a plain label when nothing is left or the server did not say", () => {
    expect(analyzeLabelFor({ fingerprinted: 10, failed: 0, pending: 0, unhashed: 0 })).toBe("Analyze library");
    expect(analyzeLabelFor(undefined)).toBe("Analyze library");
  });
});

describe("pageMatchNoteFor", () => {
  const done = { fingerprinted: 57, failed: 0, pending: 0, unhashed: 0 };
  it("says the pages were analyzed when no group rests on them", () => {
    expect(pageMatchNoteFor(done, { content: 4 })).toContain("All 57 PDFs were analyzed page by page");
  });
  it("stays quiet when a page-based group exists, while PDFs are left to analyze, or when a filter hides groups", () => {
    expect(pageMatchNoteFor(done, { pdf_resave: 1 })).toBeNull();
    expect(pageMatchNoteFor({ ...done, pending: 3 }, {})).toBeNull();
    expect(pageMatchNoteFor(done, {}, true)).toBeNull();
    expect(pageMatchNoteFor(undefined, {})).toBeNull();
  });
  it("says which PDFs could not be read", () => {
    expect(pageMatchNoteFor({ ...done, failed: 2 }, { pdf_text_match: 1 })).toBe("2 PDFs could not be read (encrypted or damaged), so they are not matched by pages.");
  });
});
