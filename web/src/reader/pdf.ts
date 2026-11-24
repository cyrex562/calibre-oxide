// PDF rendering support (issue 2.1 of the #816 epic).
//
// # Why a renderer at all
//
// PDFs could not be opened in-app at all: `is_viewable_format` in
// `lexmechanic_srv` is EPUB/KEPUB only, and the desktop webview is
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
      // The text layer's own stylesheet (positioning, transparent text,
      // selection colour). Loaded here, with the library, so it is split
      // out of the main bundle along with it.
      void import("pdfjs-dist/web/pdf_viewer.css");
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

/**
 * How a document is fetched.
 *
 * Range requests stay on (PDF.js's default) and `disableAutoFetch`
 * stops it reading the rest of the file in the background once the
 * first pages are up. On a NAS a 500 MB scan then shows page 1 after
 * the first few chunks arrive, instead of after the whole file has
 * been pulled over the network.
 */
export const PDF_LOAD_OPTIONS = {
  disableAutoFetch: true,
  disableRange: false,
  disableStream: false,
} as const;

/** The URL a book's PDF is served from. */
export function pdfUrl(bookId: string): string {
  return `/get/pdf/${bookId}`;
}

/**
 * What PDF.js rejects a cancelled render with. Thrown by [`renderPage`]
 * itself when it notices it has been superseded before drawing, so a
 * caller handles one kind of "cancelled" either way.
 */
export class RenderCancelled extends Error {
  constructor() {
    super("Rendering cancelled");
    this.name = "RenderingCancelledException";
  }
}

/** Whether `error` is a render that was cancelled on purpose, not a failure. */
export function isRenderCancelled(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { name?: unknown }).name === "RenderingCancelledException";
}

/**
 * Runs page renders one at a time, newest wins.
 *
 * A canvas cannot take two `render()` calls at once ("Cannot use the
 * same canvas during multiple render() operations"), and calling
 * `cancel()` only *asks* the old render to stop: its promise settles
 * later. So a new request cancels the running task, waits for it to
 * settle, and only then starts. A request that is itself replaced
 * while waiting never starts at all, which is what makes holding
 * Next down cheap instead of queueing every intermediate page.
 */
export class LatestRenderer {
  private token = 0;
  private task: { cancel: () => void } | null = null;
  private inflight: Promise<unknown> = Promise.resolve();

  /**
   * Runs `render`, passing a hook to register its PDF.js task and a
   * predicate telling it whether it has been superseded. Resolves
   * `true` if this call's render completed and is still the newest,
   * `false` if it was cancelled or superseded; rejects only for real
   * failures.
   */
  async run(render: (hooks: { onTask: (task: { cancel: () => void }) => void; isStale: () => boolean }) => Promise<void>): Promise<boolean> {
    const mine = ++this.token;
    this.task?.cancel();
    await this.inflight.catch(() => {});
    if (mine !== this.token) return false;

    const running = render({
      onTask: (task) => {
        this.task = task;
      },
      isStale: () => mine !== this.token,
    });
    this.inflight = running;
    try {
      await running;
    } catch (e) {
      if (isRenderCancelled(e)) return false;
      if (mine !== this.token) return false;
      throw e;
    } finally {
      if (mine === this.token) this.task = null;
    }
    return mine === this.token;
  }

  /** Cancels whatever is running and invalidates anything waiting. */
  cancel(): void {
    this.token += 1;
    this.task?.cancel();
  }
}

export interface RenderTarget {
  canvas: HTMLCanvasElement;
  scale: number;
  /** Checked after the page is fetched; when true the render is abandoned with [`RenderCancelled`]. */
  isStale?: () => boolean;
  /**
   * Called with the render task as soon as it exists, so a caller that is
   * about to show another page can `cancel()` this one (the promise then
   * rejects with PDF.js's `RenderingCancelledException`).
   */
  onTask?: (task: { cancel: () => void }) => void;
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
  if (target.isStale?.()) throw new RenderCancelled();
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

  const task = page.render({ canvas, canvasContext: context, viewport });
  target.onTask?.(task);
  await task.promise;
}

/**
 * Width a cover rendered from a page comes out at.
 *
 * Deliberately the same number as `rasterize::DEFAULT_COVER_WIDTH` on
 * the server: a cover picked in the reader and one generated at import
 * should not differ in size depending on which produced it.
 */
export const COVER_WIDTH = 1000;

export type PageImageFormat = "jpg" | "png";

export function imageMimeType(format: PageImageFormat): string {
  return format === "png" ? "image/png" : "image/jpeg";
}

/**
 * The scale at which a page `unscaledWidth` points wide renders
 * `targetWidth` pixels wide.
 *
 * Exporting and cover-picking both want a fixed output size rather
 * than whatever the reader happens to be zoomed to -- someone reading
 * at 25% should not get a 150px cover.
 */
export function scaleForWidth(unscaledWidth: number, targetWidth: number): number {
  if (!Number.isFinite(unscaledWidth) || unscaledWidth <= 0) return 1;
  if (!Number.isFinite(targetWidth) || targetWidth <= 0) return 1;
  return targetWidth / unscaledWidth;
}

/**
 * What an exported page is saved as.
 *
 * Mirrors the server's own download naming (`content.rs`: 60
 * characters, quotes and slashes replaced) so a page exported from
 * the reader and a book downloaded from the library do not follow
 * two different conventions. Backslashes are included because this
 * app runs on Windows, where they are separators too.
 */
export function pageImageFileName(title: string, page: number, format: PageImageFormat): string {
  const safe = (title || "book").slice(0, 60).replace(/["/\\]/g, "_").trim() || "book";
  return `${safe} - page ${page}.${format}`;
}

/**
 * Renders one page off-screen at a fixed width and encodes it.
 *
 * Off-screen rather than reading back the canvas already on display:
 * that one is at the reader's current zoom and device pixel ratio,
 * which is the wrong size for a cover and a surprising size for an
 * export.
 */
export async function renderPageToBlob(doc: PDFDocumentProxy, pageNumber: number, targetWidth: number, format: PageImageFormat): Promise<Blob> {
  const page = await doc.getPage(pageNumber);
  const scale = scaleForWidth(page.getViewport({ scale: 1 }).width, targetWidth);
  const viewport = page.getViewport({ scale });

  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.floor(viewport.width));
  canvas.height = Math.max(1, Math.floor(viewport.height));
  const context = canvas.getContext("2d");
  if (!context) throw new Error("could not get a 2d drawing context for the PDF page");

  // A PDF page is paper. Without this the transparent areas encode as
  // black in JPEG, which has no alpha channel -- a cover that is
  // mostly black where the page is blank.
  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, canvas.width, canvas.height);

  await page.render({ canvas, canvasContext: context, viewport }).promise;

  return await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("the page could not be encoded as an image"))), imageMimeType(format), 0.9);
  });
}
