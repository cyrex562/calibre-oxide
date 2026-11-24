import { describe, expect, it } from "vitest";

import { buildPageText, findInPage, highlightRuns, segmentsForRange } from "./pdfText";

const runs = [{ str: "The quick brown" }, { str: " fox jumps", hasEOL: true }, { str: "over the lazy dog." }];

describe("buildPageText", () => {
  it("joins runs and turns a line end into a space", () => {
    const pt = buildPageText(runs);
    expect(pt.text).toBe("The quick brown fox jumps over the lazy dog.");
    expect(pt.starts).toEqual([0, 15, 26]);
  });
});

describe("findInPage", () => {
  it("finds hits, honouring the options", () => {
    expect(findInPage(3, runs, "the", {}).map((h) => h.match.index)).toEqual([0, 31]);
    expect(findInPage(3, runs, "the", { caseSensitive: true }).map((h) => h.match.index)).toEqual([31]);
    expect(findInPage(3, runs, "jump", { wholeWord: true })).toHaveLength(0);
  });

  it("matches a phrase that wraps over a line end", () => {
    expect(findInPage(1, runs, "jumps over", {})).toHaveLength(1);
  });
});

describe("segmentsForRange", () => {
  it("maps a hit that spans two runs onto both", () => {
    const pt = buildPageText(runs);
    // "brown fox" starts in run 0 and ends in run 1.
    const start = pt.text.indexOf("brown fox");
    expect(segmentsForRange(pt, start, start + "brown fox".length)).toEqual([
      { run: 0, from: 10, to: 15 },
      { run: 1, from: 0, to: 4 },
    ]);
  });
});

describe("highlightRuns", () => {
  it("wraps hits in <mark> and marks the current one", () => {
    const divs = runs.map((r) => {
      const span = document.createElement("span");
      span.textContent = r.str;
      return span;
    });
    const hits = findInPage(1, runs, "the", {});
    highlightRuns(divs, runs, hits, 1);

    expect(divs[0].querySelectorAll("mark.pdf-hit")).toHaveLength(1);
    expect(divs[2].querySelector("mark")?.classList.contains("pdf-hit-current")).toBe(true);
    // Text survives intact, so copying still reads naturally.
    expect(divs.map((d) => d.textContent)).toEqual(runs.map((r) => r.str));
  });
});
