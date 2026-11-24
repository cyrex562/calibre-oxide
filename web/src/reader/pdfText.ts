// Text search inside a PDF page.
//
// PDF.js hands back a page's text as a list of runs. Search runs over
// the runs joined into one string (reusing the EPUB reader's
// `findMatches`), and a hit is mapped back onto the run -- and so onto
// the text-layer span -- it falls in, so it can be highlighted where it
// sits on the page.

import { findMatches, type Match, type SearchOptions } from "./search";

/** The part of a PDF.js text item this needs. */
export interface TextRun {
  str: string;
  hasEOL?: boolean;
}

export interface PageText {
  text: string;
  /** Offset in `text` at which each run starts. */
  starts: number[];
  /** Length of each run's own text (the separator added after it is not counted). */
  lengths: number[];
}

/**
 * Joins a page's runs. A line break becomes a space, so a phrase that
 * wraps still matches and two lines never fuse into one word.
 */
export function buildPageText(runs: TextRun[]): PageText {
  let text = "";
  const starts: number[] = [];
  const lengths: number[] = [];
  for (const run of runs) {
    starts.push(text.length);
    lengths.push(run.str.length);
    text += run.str;
    if (run.hasEOL) text += " ";
  }
  return { text, starts, lengths };
}

/** A hit on one page. */
export interface PdfHit {
  page: number;
  /** Position among this page's hits, in reading order. */
  index: number;
  match: Match;
}

export function findInPage(page: number, runs: TextRun[], query: string, options: SearchOptions): PdfHit[] {
  const { text } = buildPageText(runs);
  return findMatches(text, query, options).map((match, index) => ({ page, index, match }));
}

/** The slice of one run a match covers. */
export interface RunSegment {
  run: number;
  from: number;
  to: number;
}

/** Which parts of which runs the character range [start, end) covers. */
export function segmentsForRange(pt: PageText, start: number, end: number): RunSegment[] {
  const out: RunSegment[] = [];
  for (let run = 0; run < pt.starts.length; run += 1) {
    const runStart = pt.starts[run];
    const runEnd = runStart + pt.lengths[run];
    if (runEnd <= start || runStart >= end || pt.lengths[run] === 0) continue;
    out.push({ run, from: Math.max(start, runStart) - runStart, to: Math.min(end, runEnd) - runStart });
  }
  return out;
}

/**
 * Wraps the matches of one page in `<mark>` elements inside the text-layer
 * spans. `divs[i]` must be the span PDF.js made for `runs[i]`. `current`
 * (an index into `hits`) gets an extra class so it stands out.
 *
 * `<mark>`, not `<span>`: the text layer's stylesheet positions every
 * `span` absolutely.
 */
export function highlightRuns(divs: HTMLElement[], runs: TextRun[], hits: PdfHit[], current: number): void {
  const pt = buildPageText(runs);
  const perRun = new Map<number, { from: number; to: number; hit: number }[]>();
  hits.forEach((hit, hitIndex) => {
    for (const seg of segmentsForRange(pt, hit.match.index, hit.match.index + hit.match.text.length)) {
      const list = perRun.get(seg.run) ?? [];
      list.push({ from: seg.from, to: seg.to, hit: hitIndex });
      perRun.set(seg.run, list);
    }
  });

  for (const [run, segs] of perRun) {
    const div = divs[run];
    if (!div) continue;
    const str = runs[run].str;
    segs.sort((a, b) => a.from - b.from);
    div.textContent = "";
    let at = 0;
    for (const seg of segs) {
      if (seg.from < at) continue;
      if (seg.from > at) div.append(str.slice(at, seg.from));
      const mark = div.ownerDocument.createElement("mark");
      mark.className = seg.hit === current ? "pdf-hit pdf-hit-current" : "pdf-hit";
      mark.textContent = str.slice(seg.from, seg.to);
      div.append(mark);
      at = seg.to;
    }
    if (at < str.length) div.append(str.slice(at));
  }
}
