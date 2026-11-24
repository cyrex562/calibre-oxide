// Navigation helpers for the PDF reader: zoom, fit, outline and
// bookmarks. Pure logic, kept out of the component so it can be tested
// without a document or a canvas.

import type { PDFDocumentProxy } from "pdfjs-dist";

import { clampScale } from "./pdf";

// ---------------------------------------------------------------
// Zoom
// ---------------------------------------------------------------

/** "width" and "page" re-fit whenever the window changes; "custom" keeps a fixed scale. */
export type ZoomMode = "width" | "page" | "custom";

export interface ZoomState {
  mode: ZoomMode;
  /** Only meaningful for `custom`. */
  scale: number;
}

export const DEFAULT_ZOOM: ZoomState = { mode: "width", scale: 1 };
export const ZOOM_STEP = 0.25;
const ZOOM_STORAGE_KEY = "lexmechanic.pdf.zoom";

/** The scale that makes a page fit its viewing area. Never outside the zoom bounds. */
export function fitScale(mode: ZoomMode, pageWidth: number, pageHeight: number, areaWidth: number, areaHeight: number, current = 1): number {
  if (mode === "custom" || pageWidth <= 0 || pageHeight <= 0 || areaWidth <= 0 || areaHeight <= 0) return clampScale(current);
  const byWidth = areaWidth / pageWidth;
  return clampScale(mode === "width" ? byWidth : Math.min(byWidth, areaHeight / pageHeight));
}

/** Reads the remembered zoom; a missing or damaged entry means "fit width". */
export function loadZoom(): ZoomState {
  try {
    const raw = JSON.parse(localStorage.getItem(ZOOM_STORAGE_KEY) ?? "null") as Partial<ZoomState> | null;
    if (raw && (raw.mode === "width" || raw.mode === "page" || raw.mode === "custom")) {
      return { mode: raw.mode, scale: clampScale(Number(raw.scale ?? 1)) };
    }
  } catch {
    // Storage can be unavailable or hold junk; the default is fine.
  }
  return { ...DEFAULT_ZOOM };
}

export function saveZoom(zoom: ZoomState): void {
  try {
    localStorage.setItem(ZOOM_STORAGE_KEY, JSON.stringify(zoom));
  } catch {
    // Not remembering a zoom level is not worth an error.
  }
}

// ---------------------------------------------------------------
// Outline
// ---------------------------------------------------------------

/** One line of the contents panel. The page is resolved on click, not up front. */
export interface OutlineEntry {
  title: string;
  depth: number;
  dest: unknown;
  url: string | null;
}

interface RawOutlineItem {
  title: string;
  dest?: unknown;
  url?: string | null;
  items?: RawOutlineItem[];
}

/** Flattens PDF.js's nested outline into indented rows. */
export function flattenOutline(items: RawOutlineItem[] | null | undefined, depth = 0, out: OutlineEntry[] = []): OutlineEntry[] {
  for (const item of items ?? []) {
    out.push({ title: item.title?.trim() || "(untitled)", depth, dest: item.dest ?? null, url: item.url ?? null });
    if (item.items?.length) flattenOutline(item.items, depth + 1, out);
  }
  return out;
}

/**
 * The 1-based page an outline destination points at, or null.
 *
 * A destination is either a name (looked up in the document's name
 * tree) or an explicit array whose first element is a page reference
 * -- or, in some producers' files, a page index.
 */
export async function resolveDestPage(doc: Pick<PDFDocumentProxy, "getDestination" | "getPageIndex">, dest: unknown): Promise<number | null> {
  try {
    const explicit = typeof dest === "string" ? await doc.getDestination(dest) : dest;
    if (!Array.isArray(explicit) || explicit.length === 0) return null;
    const target = explicit[0];
    if (typeof target === "number") return target + 1;
    if (target && typeof target === "object") return (await doc.getPageIndex(target as never)) + 1;
  } catch {
    // A dangling destination is a broken outline entry, not a broken book.
  }
  return null;
}

// ---------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------

/**
 * The page a stored PDF bookmark points at. Bookmarks go through the
 * same annotations API as the EPUB reader's; `pos` is the page number as
 * text, the same convention the stored reading position uses.
 */
export function bookmarkPage(pos: string, pageCount: number): number | null {
  const n = Number.parseInt(pos, 10);
  if (!Number.isFinite(n) || n < 1 || (pageCount > 0 && n > pageCount)) return null;
  return n;
}
