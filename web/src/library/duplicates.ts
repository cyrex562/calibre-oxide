// What the Find duplicates panel says about a group: how strong the evidence
// is, how to filter by it, and what it is called.
//
// The server (crates/lexmechanic_srv/src/duplicates.rs) reports every group it
// finds with a `reason`, a `confidence` and the `evidence` that joined it;
// the rules for what counts as evidence are in crates/lexmechanic_db/src/dedupe.rs.
// Everything the panel needs to *decide* lives here, away from the template,
// so it can be tested.

import type { DuplicateBook, DuplicateGroup, DuplicateReason, PdfScanStatus } from "./api";

/** The weakest evidence the panel shows. */
export type DuplicateMin = "fuzzy" | "pdf" | "exact" | "identifier" | "content";

/** Strongest first. A higher rank is stronger evidence. */
const RANK: Record<DuplicateReason, number> = {
  manual: 8,
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
  manual: "groups I made by hand",
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
    case "manual":
      return { text: "Marked by you", tone: "certain" };
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

export function formatBytes(n: number): string {
  if (n >= 1024 * 1024 * 1024) return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

/**
 * The label of the action that runs the fingerprint job, or null when there is
 * nothing for it to do. `pending` is what the job would analyze; PDFs with no
 * content hash yet are read first by the same "Analyze library" job.
 */
export function fingerprintAction(status: PdfScanStatus | undefined): { label: string; note: string | null } | null {
  if (!status || status.pending === 0) return null;
  const n = status.pending.toLocaleString("en-US");
  return {
    label: `Analyze library (${n} not yet analyzed)`,
    note: status.unhashed > 0 ? `${status.unhashed.toLocaleString("en-US")} more PDFs have no content hash yet; “Analyze library” reads those first.` : null,
  };
}

/**
 * The one "prepare" button's label: it reads the files, analyzes the PDFs'
 * pages and looks for duplicates, so it says how much is still to analyze.
 */
export function analyzeLabelFor(status: PdfScanStatus | undefined): string {
  const todo = status ? status.pending + status.unhashed : 0;
  return todo > 0 ? `Analyze library (${todo.toLocaleString("en-US")} PDF${todo === 1 ? "" : "s"} not yet analyzed)` : "Analyze library";
}

const PDF_REASONS: DuplicateReason[] = ["pdf_resave", "pdf_ocr_variant", "pdf_text_match"];

/**
 * Said above the groups when every PDF has been analyzed and no group rests on
 * the pages, so an empty "same PDF, re-saved" kind is not mistaken for a step
 * that was skipped. Null while there is more to analyze, while a filter hides
 * groups, or when some page-based group exists.
 */
export function pageMatchNoteFor(status: PdfScanStatus | undefined, facets: Partial<Record<DuplicateReason, number>>, filtered = false): string | null {
  if (!status || filtered || status.pending > 0 || status.unhashed > 0) return null;
  const parts: string[] = [];
  const found = PDF_REASONS.some((r) => (facets[r] ?? 0) > 0);
  if (status.fingerprinted > 0 && !found) {
    parts.push(`All ${status.fingerprinted.toLocaleString("en-US")} PDFs were analyzed page by page and none matches another as a re-save or a scan and its OCR copy.`);
  }
  if (status.failed > 0) {
    parts.push(`${status.failed.toLocaleString("en-US")} PDF${status.failed === 1 ? "" : "s"} could not be read (encrypted or damaged), so ${status.failed === 1 ? "it is" : "they are"} not matched by pages.`);
  }
  return parts.length > 0 ? parts.join(" ") : null;
}

/** Every reason, strongest first: the order the filter chips and the resolve dialog list them in. */
export const ALL_REASONS: DuplicateReason[] = ["manual", "content", "identifier", "title_author_exact", "pdf_resave", "pdf_ocr_variant", "pdf_text_match", "title_author_fuzzy"];

/**
 * Tiers that rest on metadata alone (a shared title and author, an identifier):
 * two different books can share them, so bulk resolving never includes them
 * unless the person ticks them.
 */
export function isMetadataOnly(reason: DuplicateReason): boolean {
  return reason === "title_author_exact" || reason === "title_author_fuzzy" || reason === "identifier" || reason === "manual";
}

/** The tiers a bulk resolve starts with ticked: identical files only. */
export const DEFAULT_RESOLVE_TIERS: DuplicateReason[] = ["content"];

/** "2026-03-04 17:05" from unix seconds, or "" when unknown. */
export function formatMtime(seconds: number | null | undefined): string {
  if (!seconds) return "";
  const d = new Date(seconds * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** "EPUB 1.2 MB · PDF 40.0 MB": each format with its size. */
export function fileSummary(book: Pick<DuplicateBook, "files">): string {
  return (book.files ?? []).map((f) => `${f.format} ${formatBytes(f.size)}`).join(" · ");
}

/** The path to show for a book: its first file's path, or its folder. */
export function primaryPath(book: Pick<DuplicateBook, "files" | "path">): string {
  return book.files?.[0]?.path ?? book.path ?? "";
}

/** Whether a PDF is an image-only scan: pages but no text layer. */
export function isImageOnlyScan(book: Pick<DuplicateBook, "pdf">): boolean {
  return !!book.pdf && !book.pdf.has_text && book.pdf.scanned !== false;
}

/** "Delete" goes to the library trash, which keeps books for a while: say so. */
export function trashConfirmText(title: string): string {
  return `Move “${title}” to the library trash? It can be restored from Library → Trash… until the trash is emptied or expires.`;
}

/**
 * Cautions to show before deleting one copy from a duplicate group: the copy
 * the app suggests keeping, or the last one left (deleting it removes the
 * book from the library altogether, not a duplicate of it).
 */
export function deleteCopyWarnings(group: Pick<DuplicateGroup, "books">, book: Pick<DuplicateBook, "book_id">, keeperId: number | null): string[] {
  const out: string[] = [];
  if (group.books.length <= 1) out.push("This is the last copy left in the group. Deleting it removes the book from your library.");
  else if (keeperId !== null && keeperId === book.book_id) out.push("This is the copy suggested to keep.");
  return out;
}

/** One line for a plan's totals: "12 groups · 12 books to the trash · 1.4 GB". */
export function describePlanTotals(totals: { groups: number; books: number; bytes_freed: number }, action: "keep_best_trash_rest" | "merge_as_formats"): string {
  const n = (v: number) => v.toLocaleString("en-US");
  const books = action === "merge_as_formats" ? `${n(totals.books)} book${totals.books === 1 ? "" : "s"} merged into their keepers` : `${n(totals.books)} book${totals.books === 1 ? "" : "s"} moved to the trash`;
  return `${n(totals.groups)} group${totals.groups === 1 ? "" : "s"} · ${books} · about ${formatBytes(totals.bytes_freed)} to the Trash (freed once the Trash is emptied or expires)`;
}

/** Whether the choice of which copy to keep is the server's (a PDF group) and so worth labelling. */
export function isSuggested(group: Pick<DuplicateGroup, "suggested_dest">, book: Pick<DuplicateBook, "book_id">): boolean {
  return group.suggested_dest != null && group.suggested_dest === book.book_id;
}
