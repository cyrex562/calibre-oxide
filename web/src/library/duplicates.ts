// What the Find duplicates panel says about a group: how strong the evidence
// is, how to filter by it, and what it is called.
//
// The server (crates/calibre_srv/src/duplicates.rs) reports every group it
// finds with a `reason`, a `confidence` and the `evidence` that joined it;
// the rules for what counts as evidence are in crates/calibre_db/src/dedupe.rs.
// Everything the panel needs to *decide* lives here, away from the template,
// so it can be tested.

import type { DuplicateBook, DuplicateGroup, DuplicateReason, PdfScanStatus } from "./api";

/** The weakest evidence the panel shows. */
export type DuplicateMin = "fuzzy" | "pdf" | "exact" | "identifier" | "content";

/** Strongest first. A higher rank is stronger evidence. */
const RANK: Record<DuplicateReason, number> = {
  content: 7,
  identifier: 6,
  title_author_exact: 5,
  pdf_resave: 4,
  pdf_ocr_variant: 3,
  pdf_text_match: 2,
  title_author_fuzzy: 1,
};

// `pdf` is the page-content level: every PDF reason, and everything stronger.
const MIN_RANK: Record<DuplicateMin, number> = { content: 7, identifier: 6, exact: 5, pdf: 2, fuzzy: 1 };

export const MIN_OPTIONS: { value: DuplicateMin; label: string }[] = [
  { value: "fuzzy", label: "Everything, including probable matches" },
  { value: "pdf", label: "Same PDF pages or text, and stronger" },
  { value: "exact", label: "Same title and author, and stronger" },
  { value: "identifier", label: "Same ISBN or identifier, and stronger" },
  { value: "content", label: "Identical files only" },
];

const LABELS: Record<DuplicateReason, string> = {
  content: "identical files",
  identifier: "same ISBN or identifier",
  title_author_exact: "same title and author",
  title_author_fuzzy: "probably the same book",
  pdf_resave: "same PDF, re-saved",
  pdf_ocr_variant: "same scan, with and without OCR text",
  pdf_text_match: "same text, different pages",
};

export function reasonLabel(reason: DuplicateReason): string {
  return LABELS[reason] ?? reason;
}

export type BadgeTone = "certain" | "strong" | "likely" | "review";

/** The short badge for a reason. `review` means a person must confirm. */
export function reasonBadge(reason: DuplicateReason): { text: string; tone: BadgeTone } {
  switch (reason) {
    case "content":
      return { text: "Identical file", tone: "certain" };
    case "identifier":
      return { text: "Same ISBN / ID", tone: "strong" };
    case "title_author_exact":
      return { text: "Same title + author", tone: "likely" };
    case "pdf_resave":
      return { text: "Re-saved PDF", tone: "likely" };
    case "pdf_ocr_variant":
      return { text: "Scan vs OCR", tone: "likely" };
    case "pdf_text_match":
      return { text: "Same text", tone: "review" };
    default:
      return { text: "Possible match", tone: "review" };
  }
}

/** 0.9 -> "90%". */
export function confidencePercent(confidence: number): string {
  return `${Math.round(confidence * 100)}%`;
}

/**
 * Whether the group is a guess. Only an identical file or a shared identifier
 * is decided by rule; a title match, however exact, can be two books that
 * share a title and an author's name.
 */
export function needsReview(group: Pick<DuplicateGroup, "reason">): boolean {
  return group.reason === "title_author_fuzzy" || group.reason === "pdf_text_match";
}

/** Groups at least as strong as `min`. Order is kept. */
export function filterGroups<G extends Pick<DuplicateGroup, "reason">>(groups: G[], min: DuplicateMin): G[] {
  const floor = MIN_RANK[min] ?? 1;
  return groups.filter((g) => (RANK[g.reason] ?? 0) >= floor);
}

/**
 * How many groups each filter would show, keyed by the filter's value, so the
 * dropdown can say what choosing it does.
 */
export function countByReason(groups: Pick<DuplicateGroup, "reason">[]): Record<DuplicateMin, number> {
  const out: Record<DuplicateMin, number> = { fuzzy: 0, pdf: 0, exact: 0, identifier: 0, content: 0 };
  for (const min of Object.keys(out) as DuplicateMin[]) {
    out[min] = filterGroups(groups, min).length;
  }
  return out;
}

/** One line per piece of evidence, for a tooltip. */
export function evidenceLines(group: Pick<DuplicateGroup, "evidence">): string[] {
  return (group.evidence ?? []).slice(0, 8).map((e) => `${reasonLabel(e.reason)} (${confidencePercent(e.confidence)}): ${e.detail}`);
}

/**
 * What a PDF in a group is, so the user can choose between a scan and its
 * OCR: "312 pages · text layer" or "312 pages · image only". Empty for a book
 * that is not a fingerprinted PDF.
 */
export function pdfLine(book: Pick<DuplicateBook, "pdf">): string {
  const pdf = book.pdf;
  if (!pdf) return "";
  const pages = `${pdf.pages.toLocaleString("en-US")} page${pdf.pages === 1 ? "" : "s"}`;
  return `${pages} · ${pdf.has_text ? "text layer" : "image only"} · ${formatBytes(pdf.size)}`;
}

function formatBytes(n: number): string {
  if (n >= 1024 * 1024 * 1024) return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

/**
 * The label of the action that runs the fingerprint job, or null when there is
 * nothing for it to do. `pending` is what the job would analyse; PDFs with no
 * content hash yet need "Prepare for duplicate detection" first.
 */
export function fingerprintAction(status: PdfScanStatus | undefined): { label: string; note: string | null } | null {
  if (!status || status.pending === 0) return null;
  const n = status.pending.toLocaleString("en-US");
  return {
    label: `Fingerprint PDFs (${n} not yet analysed)`,
    note: status.unhashed > 0 ? `${status.unhashed.toLocaleString("en-US")} more PDFs have no content hash yet; use “Prepare for duplicate detection” for those.` : null,
  };
}

/** Whether the choice of which copy to keep is the server's (a PDF group) and so worth labelling. */
export function isSuggested(group: Pick<DuplicateGroup, "suggested_dest">, book: Pick<DuplicateBook, "book_id">): boolean {
  return group.suggested_dest != null && group.suggested_dest === book.book_id;
}
