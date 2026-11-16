import { describe, expect, it } from "vitest";
import { MIN_OPTIONS, confidencePercent, countByReason, evidenceLines, filterGroups, fingerprintAction, isSuggested, needsReview, pdfLine, reasonBadge, reasonLabel } from "./duplicates";
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
  it("offers the job with the count of PDFs not yet analysed", () => {
    expect(fingerprintAction({ fingerprinted: 10, failed: 0, pending: 1234, unhashed: 0 })).toEqual({ label: "Fingerprint PDFs (1,234 not yet analysed)", note: null });
  });
  it("says what to do about PDFs that have no hash yet", () => {
    expect(fingerprintAction({ fingerprinted: 0, failed: 0, pending: 5, unhashed: 7 })?.note).toContain("Prepare for duplicate detection");
  });
  it("offers nothing when everything is analysed, or the server did not say", () => {
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
