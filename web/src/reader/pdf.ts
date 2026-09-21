// PDF rendering support (issue 2.1 of the #816 epic).
//
// # Why a renderer at all
//
// PDFs could not be opened in-app at all: `is_viewable_format` in
// `calibre_srv` is EPUB/KEPUB only, and the desktop webview is
// WebKitGTK, which ships no built-in PDF viewer -- so `<embed>` or an
// iframe, which would work in Chrome or Firefox, renders nothing in
// the app this is actually for. For a library whose primary content
// is PDFs, that made the reader useless.
//
// # Why PDF.js is loaded lazily
//
// The library is large -- far bigger than the rest of the frontend
// bundle put together. A static import would make every user pay for
// it on first load whether or not they ever open a PDF. `loadPdfJs`
// dynamically imports it, which Vite code-splits automatically, so it
// arrives only when a PDF is actually opened.
//
// # Why the worker is wired explicitly
//
// PDF.js parses in a worker. Its default worker URL points at a CDN,
// which the app's CSP blocks outright -- and would be wrong anyway
// for something that must work offline. The worker is resolved
// through `import.meta.url` so the bundler emits a local copy.

import type { PDFDocumentProxy } from "pdfjs-dist";

/** Bounds for the zoom control. Outside these a page is unusable. */
export const MIN_SCALE = 0.25;
export const MAX_SCALE = 5;

export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/**
 * Keeps a page number inside a document.
 *
 * Pages are 1-based, which is the off-by-one this exists to contain:
 * a 0 or an `n+1` reaches PDF.js as a rejected promise rather than a
 * clamped page, and the reader would show an error instead of an
 * edge.
 */
export function clampPage(page: number, pageCount: number): number {
  if (!Number.isFinite(page) || pageCount < 1) return 1;
  return Math.min(pageCount, Math.max(1, Math.floor(page)));
}

let pdfjsPromise: Promise<typeof import("pdfjs-dist")> | null = null;

/**
 * Loads PDF.js once, wiring its worker to a bundled copy.
 *
 * Memoized because the reader can open several PDFs in a session and
 * re-importing would re-run the module's own setup each time.
 */
export function loadPdfJs(): Promise<typeof import("pdfjs-dist")> {
  if (!pdfjsPromise) {
    pdfjsPromise = import("pdfjs-dist").then((pdfjs) => {
      pdfjs.GlobalWorkerOptions.workerSrc = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).href;
      return pdfjs;
    });
  }
  return pdfjsPromise;
}

/**
 * Where PDF.js finds its runtime data, copied into `public/pdfjs/` by
 * `scripts/copy-pdfjs-assets.mjs`.
 *
 * `standardFontDataUrl` is not optional in practice: a PDF that names
 * one of the 14 standard fonts carries no glyphs for it, so without
 * this PDF.js warns "Ensure that the standardFontDataUrl API
 * parameter is provided" and the text does not render properly.
 * Verified against a PDF produced by this project's own
 * `ebook_convert`, which uses those fonts.
 *
 * `cMapUrl` is the same story for CJK encodings.
 *
 * Both are served on demand -- a reader fetches only the specific
 * font or cmap a given PDF asks for -- so they cost disk, not load
 * time.
 */
export const PDF_ASSET_OPTIONS = {
  standardFontDataUrl: "/pdfjs/standard_fonts/",
  cMapUrl: "/pdfjs/cmaps/",
  cMapPacked: true,
} as const;

/** The URL a book's PDF is served from. */
export function pdfUrl(bookId: string): string {
  return `/get/pdf/${bookId}`;
}

export interface RenderTarget {
  canvas: HTMLCanvasElement;
  scale: number;
}

/**
 * Renders one page onto a canvas at `scale`, accounting for device
 * pixel ratio.
 *
 * Without the ratio adjustment a page is visibly blurry on any
 * high-DPI screen -- the canvas would be sized in CSS pixels and
 * upscaled by the compositor rather than rendered at native
 * resolution.
 */
export async function renderPage(doc: PDFDocumentProxy, pageNumber: number, target: RenderTarget): Promise<void> {
  const page = await doc.getPage(pageNumber);
  const ratio = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
  const viewport = page.getViewport({ scale: target.scale * ratio });

  const canvas = target.canvas;
  canvas.width = Math.floor(viewport.width);
  canvas.height = Math.floor(viewport.height);
  // CSS size stays in logical pixels so layout is unaffected by DPI.
  canvas.style.width = `${Math.floor(viewport.width / ratio)}px`;
  canvas.style.height = `${Math.floor(viewport.height / ratio)}px`;

  const context = canvas.getContext("2d");
  if (!context) throw new Error("could not get a 2d drawing context for the PDF page");

  await page.render({ canvas, canvasContext: context, viewport }).promise;
}
