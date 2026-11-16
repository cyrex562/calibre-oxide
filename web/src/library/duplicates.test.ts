import { describe, expect, it } from "vitest";
import { MIN_OPTIONS, confidencePercent, countByReason, evidenceLines, filterGroups, needsReview, reasonBadge, reasonLabel } from "./duplicates";
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
    expect(countByReason(groups)).toEqual({ fuzzy: 5, exact: 3, identifier: 2, content: 1 });
  });
  it("is zero for an empty scan", () => {
    expect(countByReason([])).toEqual({ fuzzy: 0, exact: 0, identifier: 0, content: 0 });
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
  it("lists the four filters, weakest first, with the default first", () => {
    expect(MIN_OPTIONS.map((o) => o.value)).toEqual(["fuzzy", "exact", "identifier", "content"]);
  });
});
