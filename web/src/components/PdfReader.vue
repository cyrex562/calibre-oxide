<script setup lang="ts">
// Inline PDF reading (issue 2.1 of the #816 epic).
//
// The desktop webview is WebKitGTK, which ships no built-in PDF
// viewer, so an `<embed>` or iframe -- which would work in Chrome or
// Firefox -- renders nothing in the app this is actually for. PDF.js
// is the only route to showing a PDF in-app.
//
// Position is remembered with the same `/book-set-last-read-position`
// mechanism the EPUB reader uses, so reopening a PDF lands where you
// left off. That is the whole reason to read in-app rather than
// handing the file to a system viewer, which cannot report back.
//
// Beyond showing pages it is a reader: a contents panel from the PDF's
// outline, a text layer (select and copy), in-document search, fit
// width / fit page, bookmarks, and the keys people expect.

import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import type { TextContent } from "pdfjs-dist/types/src/display/api";

import { clampPage, clampScale, COVER_WIDTH, LatestRenderer, loadPdfJs, MAX_SCALE, MIN_SCALE, PDF_ASSET_OPTIONS, PDF_LOAD_OPTIONS, pageImageFileName, type PageImageFormat, pdfUrl, RenderCancelled, renderPage, renderPageToBlob } from "../reader/pdf";
import { bookmarkPage, fitScale, flattenOutline, loadZoom, type OutlineEntry, resolveDestPage, saveZoom, type ZoomMode, type ZoomState, ZOOM_STEP } from "../reader/pdfNav";
import { findInPage, highlightRuns, type PdfHit, type TextRun } from "../reader/pdfText";
import { addBookmark, getAnnotations, getLastReadPositions, removeAnnotation, setLastReadPosition } from "../reader/api";
import { deviceId } from "../reader/position";
import type { Bookmark } from "../reader/types";
import { coverFromPdfPage, fetchBook, setCover } from "../library/api";
import { humanError } from "../library/errorText";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import ReaderToolbar from "./ReaderToolbar.vue";
import { promptDialog } from "./dialogs/confirm";

const props = defineProps<{ bookId: string }>();

// The title is fetched here anyway, to name an exported page file after
// the book; the parent can show it too.
const emit = defineEmits<{ title: [value: string] }>();

const canvas = ref<HTMLCanvasElement | null>(null);
const areaEl = ref<HTMLElement | null>(null);
const textLayerEl = ref<HTMLElement | null>(null);
const boxEl = ref<HTMLElement | null>(null);
const findInput = ref<HTMLInputElement | null>(null);
// `shallowRef`, not `ref`: a deep reactive proxy over a PDF document
// both reshapes its type (Vue's unwrapping strips the class's private
// fields, so it no longer satisfies `PDFDocumentProxy`) and would
// walk a very large object graph for no benefit. Nothing here needs
// reactivity *inside* the document -- only on which document it is.
const doc = shallowRef<PDFDocumentProxy | null>(null);
const pageCount = ref(0);
const page = ref(1);
const zoom = ref<ZoomState>(loadZoom());
// The scale actually on screen: for the fit modes, whatever fits.
const shownScale = ref(1);
const loading = ref(true);
const error = ref<string | null>(null);
const title = ref("");

type Panel = "contents" | "bookmarks" | null;
const panel = ref<Panel>(null);
const outline = shallowRef<OutlineEntry[]>([]);
const bookmarks = ref<Bookmark[]>([]);

// Transient feedback for the actions below. They finish quickly and have
// no visible result inside the reader -- a new cover shows up in the
// library, and a download lands in the browser's own UI -- so without a
// line of text it is not obvious anything happened.
const notice = ref<string | null>(null);
let noticeTimer: ReturnType<typeof setTimeout> | undefined;
function say(message: string) {
  notice.value = message;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = null), 4000);
}

const busy = ref(false);
const exportMenu = ref<{ x: number; y: number } | null>(null);

const EXPORT_FORMATS: ContextMenuEntry<PageImageFormat>[] = [
  { id: "jpg", label: "JPEG image (.jpg)", enabled: true },
  { id: "png", label: "PNG image (.png)", enabled: true },
];

function openExportMenu(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  exportMenu.value = { x: rect.left, y: rect.bottom };
}

/**
 * Stores the page on screen as the book's cover.
 *
 * Rendered fresh at `COVER_WIDTH` rather than read back off the
 * displayed canvas, which is at whatever zoom the reader is at.
 *
 * Falls back to rendering it server-side if encoding the canvas
 * fails. `canvas.toBlob` is the one step here with no coverage: the
 * desktop app runs on WebKitGTK, which cannot be driven headlessly on
 * the machine this is developed on, so its JPEG encoding is untested
 * in the browser that matters most. The fallback costs three lines and
 * uses a route that exists anyway.
 */
async function useAsCover() {
  const d = doc.value;
  if (!d || busy.value) return;
  busy.value = true;
  try {
    let blob: Blob | null = null;
    try {
      blob = await renderPageToBlob(d, page.value, COVER_WIDTH, "jpg");
    } catch {
      blob = null;
    }
    if (blob) await setCover(Number(props.bookId), new File([blob], "cover.jpg", { type: "image/jpeg" }));
    else await coverFromPdfPage(Number(props.bookId), page.value);
    say(`Page ${page.value} is now the cover.`);
  } catch (e) {
    say(`Could not set the cover: ${humanError(e)}`);
  } finally {
    busy.value = false;
  }
}

/** Saves the page on screen as an image file. */
async function exportPage(format: PageImageFormat) {
  exportMenu.value = null;
  const d = doc.value;
  if (!d || busy.value) return;
  busy.value = true;
  try {
    const blob = await renderPageToBlob(d, page.value, COVER_WIDTH, format);
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = pageImageFileName(title.value, page.value, format);
    link.click();
    // Not revoked synchronously: Chromium and WebKit both start the
    // download asynchronously after the click, and revoking first
    // cancels it.
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
    say(`Saved ${link.download}.`);
  } catch (e) {
    say(`Could not export the page: ${humanError(e)}`);
  } finally {
    busy.value = false;
  }
}

// ---------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------

// A canvas takes one render at a time, and `cancel()` only asks the old
// one to stop. `LatestRenderer` cancels, waits for the old task to
// settle, and drops a request that was itself replaced meanwhile -- so
// clicking Next five times quickly ends on the fifth page rather than a
// blank canvas and PDF.js's "same canvas" error.
const renderer = new LatestRenderer();

async function pageRuns(d: PDFDocumentProxy, n: number, keep: boolean): Promise<{ runs: TextRun[]; content: TextContent }> {
  const p = await d.getPage(n);
  const content = await p.getTextContent();
  // One run per item that has text, in the order PDF.js draws a span for each.
  const runs: TextRun[] = [];
  for (const item of content.items) if ("str" in item) runs.push({ str: item.str, hasEOL: item.hasEOL });
  // A page only scanned for search is not about to be drawn: let PDF.js free it.
  if (!keep) p.cleanup();
  return { runs, content };
}

let textLayer: { cancel: () => void } | null = null;
let textDivs: HTMLElement[] = [];
let textRuns: TextRun[] = [];
let textLayerPage = 0;
let textToken = 0;

async function drawTextLayer(d: PDFDocumentProxy, n: number, scale: number) {
  const container = textLayerEl.value;
  const box = boxEl.value;
  if (!container || !box) return;
  const token = ++textToken;
  textLayer?.cancel();
  textLayer = null;
  textDivs = [];
  textRuns = [];
  textLayerPage = 0;
  container.textContent = "";
  box.style.setProperty("--total-scale-factor", String(scale));
  try {
    const pdfjs = await loadPdfJs();
    const p = await d.getPage(n);
    const { runs, content } = await pageRuns(d, n, true);
    if (token !== textToken) return;
    const layer = new pdfjs.TextLayer({ textContentSource: content, container, viewport: p.getViewport({ scale }) });
    textLayer = layer;
    await layer.render();
    if (token !== textToken) return;
    textDivs = layer.textDivs;
    textRuns = runs;
    textLayerPage = n;
    applyHighlights();
  } catch {
    // The text layer is a convenience on top of the page image: a
    // page whose text cannot be read is still a readable page.
  }
}

async function draw() {
  const d = doc.value;
  const el = canvas.value;
  if (!d || !el) return;
  const n = page.value;
  try {
    let scale = 1;
    const done = await renderer.run(async ({ onTask, isStale }) => {
      const p = await d.getPage(n);
      if (isStale()) throw new RenderCancelled();
      const natural = p.getViewport({ scale: 1 });
      const area = areaEl.value;
      // 2rem of padding either side in the page area.
      scale = fitScale(zoom.value.mode, natural.width, natural.height, (area?.clientWidth ?? 0) - 32, (area?.clientHeight ?? 0) - 32, zoom.value.scale);
      shownScale.value = scale;
      await renderPage(d, n, { canvas: el, scale, onTask, isStale });
    });
    if (!done) return; // superseded or cancelled: the newer render owns the canvas
    error.value = null;
    void drawTextLayer(d, n, scale);
  } catch (e) {
    error.value = humanError(e);
  }
}

async function load() {
  loading.value = true;
  error.value = null;
  renderer.cancel();
  outline.value = [];
  bookmarks.value = [];
  hits.value = [];
  hitIndex.value = -1;
  try {
    const pdfjs = await loadPdfJs();
    const task = pdfjs.getDocument({ url: pdfUrl(props.bookId), ...PDF_ASSET_OPTIONS, ...PDF_LOAD_OPTIONS });
    const loaded = await task.promise;
    void doc.value?.destroy();
    doc.value = loaded;
    pageCount.value = loaded.numPages;

    // Not fatal: the title only names an exported file and fills the
    // header, and neither is worth failing to open a book over.
    void fetchBook(Number(props.bookId))
      .then((book) => {
        title.value = book.title ?? "";
        emit("title", title.value);
      })
      .catch(() => {});

    // Not fatal either, and not awaited: neither should hold up page 1.
    void loaded
      .getOutline()
      .then((o) => {
        if (doc.value === loaded) outline.value = flattenOutline(o as never);
      })
      .catch(() => {});
    void getAnnotations(props.bookId, "pdf")
      .then((map) => {
        if (doc.value === loaded) bookmarks.value = (map.bookmark ?? []).filter((b) => !b.removed);
      })
      .catch(() => {});

    // Restore where we were, if anything was stored.
    let start = 1;
    try {
      // Positions come back one per device; prefer this device's own.
      const positions = await getLastReadPositions(props.bookId, "pdf");
      const mine = positions.find((p) => p.device === deviceId()) ?? positions[0];
      const restored = mine?.cfi ? Number.parseInt(mine.cfi, 10) : Number.NaN;
      start = clampPage(restored, loaded.numPages);
    } catch {
      start = 1;
    }
    page.value = start;
    // The one and only draw of a freshly opened book.
    loading.value = false;
    await nextTick();
    await draw();
  } catch (e) {
    error.value = humanError(e);
  } finally {
    loading.value = false;
  }
}

// The page number doubles as the stored position: PDFs have no CFI,
// and a page is the only location a reader and a file agree on.
// Debounced so holding a key through a long book is not one request
// per page.
let saveTimer: ReturnType<typeof setTimeout> | undefined;
function savePosition() {
  clearTimeout(saveTimer);
  const value = page.value;
  const total = pageCount.value;
  saveTimer = setTimeout(() => {
    void setLastReadPosition(props.bookId, "pdf", deviceId(), String(value), total > 1 ? (value - 1) / (total - 1) : 0).catch(() => {});
  }, 400);
}

function showPage(n: number) {
  const next = clampPage(n, pageCount.value);
  if (next === page.value && textLayerPage === next) return;
  page.value = next;
  savePosition();
  void draw();
}

function go(delta: number) {
  showPage(page.value + delta);
}

function setPage(value: number) {
  showPage(Number.isFinite(value) ? value : page.value);
}

// ---------------------------------------------------------------
// Zoom
// ---------------------------------------------------------------

function applyZoom(next: ZoomState) {
  zoom.value = next;
  saveZoom(next);
  void draw();
}

function fit(mode: ZoomMode) {
  applyZoom({ mode, scale: shownScale.value });
}

function zoomBy(delta: number) {
  applyZoom({ mode: "custom", scale: clampScale(Math.round((shownScale.value + delta) * 100) / 100) });
}

function onWheel(event: WheelEvent) {
  if (!event.ctrlKey) return;
  event.preventDefault();
  zoomBy(event.deltaY < 0 ? 0.1 : -0.1);
}

// In a fit mode the right scale depends on the window, so a resize has
// to re-fit; a fixed zoom is left alone.
let resizeTimer: ReturnType<typeof setTimeout> | undefined;
let resizeObserver: ResizeObserver | null = null;
function onAreaResize() {
  if (zoom.value.mode === "custom" || !doc.value) return;
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => void draw(), 120);
}

// ---------------------------------------------------------------
// Contents and bookmarks
// ---------------------------------------------------------------

function togglePanel(which: Exclude<Panel, null>) {
  panel.value = panel.value === which ? null : which;
}

async function openOutlineEntry(entry: OutlineEntry) {
  const d = doc.value;
  if (!d) return;
  const target = await resolveDestPage(d, entry.dest);
  if (target) showPage(target);
  else say("That contents entry does not point at a page.");
}

async function bookmarkThisPage() {
  const n = page.value;
  const entered = await promptDialog("Bookmark title:", `Page ${n}`, { title: "Add bookmark", confirmLabel: "Add" });
  if (!entered) return;
  try {
    const bookmark = await addBookmark(props.bookId, "pdf", entered, String(n));
    // Re-bookmarking under the same title replaces it server-side.
    bookmarks.value = [...bookmarks.value.filter((b) => b.title !== entered), bookmark];
    say(`Bookmarked page ${n}.`);
  } catch (e) {
    say(`Could not add the bookmark: ${humanError(e)}`);
  }
}

function openBookmark(bookmark: Bookmark) {
  const n = bookmarkPage(bookmark.pos, pageCount.value);
  if (n) showPage(n);
}

async function deleteBookmark(bookmark: Bookmark) {
  try {
    await removeAnnotation(props.bookId, "pdf", "bookmark", bookmark);
    bookmarks.value = bookmarks.value.filter((b) => b.title !== bookmark.title);
  } catch (e) {
    say(`Could not remove the bookmark: ${humanError(e)}`);
  }
}

// ---------------------------------------------------------------
// Search
// ---------------------------------------------------------------
//
// Every page's text is read in order and the hits stream in, so a hit
// near the start shows up while a long scan is still running. A hit is
// shown by highlighting it in the text layer of its page.

const findOpen = ref(false);
const query = ref("");
const wholeWord = ref(false);
const matchCase = ref(false);
const hits = shallowRef<PdfHit[]>([]);
const hitIndex = ref(-1);
const scanning = ref(false);
const scanned = ref(0);
let searchedKey = "";
let scanRun = 0;

const searchKey = () => JSON.stringify([query.value.trim(), wholeWord.value, matchCase.value]);

async function runSearch() {
  const d = doc.value;
  const text = query.value.trim();
  const run = ++scanRun;
  hits.value = [];
  hitIndex.value = -1;
  scanned.value = 0;
  searchedKey = searchKey();
  applyHighlights();
  if (!d || !text) {
    scanning.value = false;
    return;
  }
  scanning.value = true;
  const options = { wholeWord: wholeWord.value, caseSensitive: matchCase.value };
  const startPage = page.value;
  try {
    for (let n = 1; n <= d.numPages; n += 1) {
      if (run !== scanRun) return;
      try {
        const { runs } = await pageRuns(d, n, false);
        if (run !== scanRun) return;
        const found = findInPage(n, runs, text, options);
        if (found.length) {
          hits.value = [...hits.value, ...found];
          // Land on the first hit at or after the page being read.
          if (hitIndex.value < 0 && n >= startPage) selectHit(hits.value.length - found.length);
        }
      } catch {
        // One unreadable page must not abandon the rest of the book.
      }
      scanned.value = n;
    }
    if (run === scanRun && hitIndex.value < 0 && hits.value.length) selectHit(0);
  } finally {
    if (run === scanRun) scanning.value = false;
  }
}

function selectHit(index: number) {
  const hit = hits.value[index];
  if (!hit) return;
  hitIndex.value = index;
  if (hit.page !== page.value) showPage(hit.page);
  else applyHighlights();
}

function findStep(direction: 1 | -1) {
  const total = hits.value.length;
  if (!total) return;
  selectHit((hitIndex.value + direction + total) % total);
}

function onFindEnter(direction: 1 | -1) {
  if (searchKey() !== searchedKey) void runSearch();
  else findStep(direction);
}

function openFind() {
  findOpen.value = true;
  void nextTick(() => {
    findInput.value?.focus();
    findInput.value?.select();
  });
}

function closeFind() {
  findOpen.value = false;
  scanRun += 1;
  scanning.value = false;
  hits.value = [];
  hitIndex.value = -1;
  searchedKey = "";
  applyHighlights();
}

function toggleFind() {
  if (findOpen.value) closeFind();
  else openFind();
}

let markedRuns = new Set<number>();
function applyHighlights() {
  // Undo the previous highlighting first, or a stale mark would survive a new query.
  for (const run of markedRuns) {
    if (textDivs[run] && textRuns[run]) textDivs[run].textContent = textRuns[run].str;
  }
  markedRuns = new Set();
  if (!findOpen.value || !textDivs.length || textLayerPage !== page.value) return;

  const onPage = hits.value.filter((h) => h.page === page.value);
  if (!onPage.length) return;
  const current = hits.value[hitIndex.value];
  const local = current && current.page === page.value ? current.index : -1;
  highlightRuns(textDivs, textRuns, onPage, local);
  textDivs.forEach((div, i) => {
    if (div.querySelector("mark")) markedRuns.add(i);
  });
  void nextTick(() => textLayerEl.value?.querySelector<HTMLElement>(".pdf-hit-current")?.scrollIntoView?.({ block: "center", inline: "nearest" }));
}

watch(
  () => hits.value.length,
  () => applyHighlights(),
);
watch([wholeWord, matchCase], () => {
  if (findOpen.value && query.value.trim()) void runSearch();
});

const findStatus = computed(() => {
  if (!query.value.trim()) return "";
  if (scanning.value) return `${hits.value.length ? `${hitIndex.value + 1} of ${hits.value.length}+ · ` : ""}searching… page ${scanned.value} of ${pageCount.value}`;
  if (searchedKey !== searchKey()) return "Press Enter to search";
  if (!hits.value.length) return "No matches";
  return `${hitIndex.value + 1} of ${hits.value.length} matches`;
});

// ---------------------------------------------------------------
// Keys
// ---------------------------------------------------------------

function onKeydown(event: KeyboardEvent) {
  // The reader is not the library, so it may own Ctrl+F.
  if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "f") {
    event.preventDefault();
    openFind();
    return;
  }
  const target = event.target;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
  if (event.ctrlKey || event.metaKey || event.altKey) return;

  switch (event.key) {
    case "ArrowRight":
    case "PageDown":
      go(1);
      break;
    case "ArrowLeft":
    case "PageUp":
      go(-1);
      break;
    case "Home":
      showPage(1);
      break;
    case "End":
      showPage(pageCount.value);
      break;
    case "+":
    case "=":
      zoomBy(ZOOM_STEP);
      break;
    case "-":
      zoomBy(-ZOOM_STEP);
      break;
    case "Escape":
      if (!findOpen.value) return;
      closeFind();
      break;
    default:
      return;
  }
  event.preventDefault();
}

watch(
  () => props.bookId,
  () => void load(),
);

const atMinZoom = computed(() => shownScale.value <= MIN_SCALE + 1e-6);
const atMaxZoom = computed(() => shownScale.value >= MAX_SCALE - 1e-6);
const sidebarLabel = computed(() => (panel.value === "contents" ? "Contents" : "Bookmarks"));

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  if (typeof ResizeObserver !== "undefined" && areaEl.value) {
    resizeObserver = new ResizeObserver(onAreaResize);
    resizeObserver.observe(areaEl.value);
  }
  void load();
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  resizeObserver?.disconnect();
  clearTimeout(noticeTimer);
  clearTimeout(resizeTimer);
  clearTimeout(saveTimer);
  scanRun += 1;
  renderer.cancel();
  textLayer?.cancel();
  // Frees the worker's copy of the document; without this, opening
  // several PDFs in a session leaks each one.
  void doc.value?.destroy();
});
</script>

<template>
  <div class="pdf-reader">
    <ReaderToolbar
      :title="title || 'PDF'"
      :disabled="!doc"
      :bookmark-count="bookmarks.length"
      :contents-active="panel === 'contents'"
      :bookmarks-active="panel === 'bookmarks'"
      :search-active="findOpen"
      :can-prev="page > 1"
      :can-next="pageCount > 0 && page < pageCount"
      @contents="togglePanel('contents')"
      @bookmarks="togglePanel('bookmarks')"
      @search="toggleFind"
      @bookmark="bookmarkThisPage"
      @prev="go(-1)"
      @next="go(1)"
    >
      <template #position>
        <label class="pdf-page">
          Page
          <input type="number" aria-label="Page number" :value="page" min="1" :max="pageCount || 1" @change="setPage(Number(($event.target as HTMLInputElement).value))" />
          <span>/ {{ pageCount || "?" }}</span>
        </label>
      </template>
      <template #extra>
        <span class="pdf-zoom" role="group" aria-label="Zoom">
          <button type="button" :class="{ active: zoom.mode === 'width' }" :aria-pressed="zoom.mode === 'width'" title="Fit the page to the window width" @click="fit('width')">Fit width</button>
          <button type="button" :class="{ active: zoom.mode === 'page' }" :aria-pressed="zoom.mode === 'page'" title="Fit the whole page in the window" @click="fit('page')">Fit page</button>
          <button type="button" title="Zoom out (-)" aria-label="Zoom out" :disabled="atMinZoom" @click="zoomBy(-ZOOM_STEP)">−</button>
          <span class="pdf-scale" aria-live="polite">{{ Math.round(shownScale * 100) }}%</span>
          <button type="button" title="Zoom in (+)" aria-label="Zoom in" :disabled="atMaxZoom" @click="zoomBy(ZOOM_STEP)">+</button>
        </span>
        <span class="pdf-sep" aria-hidden="true" />
        <button type="button" :disabled="!doc || busy" title="Store this page as the book's cover" @click="useAsCover">Use as cover</button>
        <button type="button" :disabled="!doc || busy" title="Save this page as an image file" @click="openExportMenu">Export page…</button>
      </template>
    </ReaderToolbar>

    <div v-if="findOpen" class="pdf-find" role="search">
      <input
        ref="findInput"
        v-model="query"
        type="search"
        aria-label="Search in this PDF"
        placeholder="Find in this PDF…"
        @keydown.enter.prevent="onFindEnter(($event as KeyboardEvent).shiftKey ? -1 : 1)"
        @keydown.esc.prevent="closeFind"
      />
      <button type="button" :disabled="!hits.length" title="Previous match (Shift+Enter)" @click="findStep(-1)">▲ Previous</button>
      <button type="button" :disabled="!query.trim()" title="Next match (Enter)" @click="onFindEnter(1)">▼ Next</button>
      <label><input v-model="wholeWord" type="checkbox" /> Whole word</label>
      <label><input v-model="matchCase" type="checkbox" /> Match case</label>
      <span class="pdf-find-status" aria-live="polite">{{ findStatus }}</span>
      <button type="button" aria-label="Close search" title="Close (Esc)" @click="closeFind">✕</button>
    </div>

    <ContextMenu v-if="exportMenu" :x="exportMenu.x" :y="exportMenu.y" :entries="EXPORT_FORMATS" @choose="exportPage" @close="exportMenu = null" />

    <p v-if="notice" class="notice" role="status">{{ notice }}</p>

    <p v-if="loading" class="status">Loading PDF…</p>
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <div class="pdf-body">
      <aside v-if="panel" class="pdf-side" :aria-label="sidebarLabel">
        <template v-if="panel === 'contents'">
          <p v-if="!outline.length" class="pdf-side-empty">This PDF has no table of contents.</p>
          <ul v-else>
            <li v-for="(entry, i) in outline" :key="i" :style="{ paddingLeft: `${entry.depth * 0.9}em` }">
              <button type="button" class="pdf-link" @click="openOutlineEntry(entry)">{{ entry.title }}</button>
            </li>
          </ul>
        </template>
        <template v-else>
          <p v-if="!bookmarks.length" class="pdf-side-empty">No bookmarks yet. Use "Bookmark this page".</p>
          <ul v-else>
            <li v-for="b in bookmarks" :key="b.title" class="pdf-bookmark">
              <button type="button" class="pdf-link" @click="openBookmark(b)">{{ b.title }}</button>
              <button type="button" class="pdf-remove" :aria-label="`Remove bookmark ${b.title}`" title="Remove this bookmark" @click="deleteBookmark(b)">✕</button>
            </li>
          </ul>
        </template>
      </aside>

      <div ref="areaEl" class="pdf-page-area" @wheel="onWheel">
        <div ref="boxEl" class="pdf-page-box">
          <canvas ref="canvas" />
          <div ref="textLayerEl" class="textLayer" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pdf-reader {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.pdf-page {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  font-size: var(--fs-small);
}
.pdf-page input {
  width: 5rem;
  font-variant-numeric: tabular-nums;
}
.pdf-zoom {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
}
.pdf-zoom button.active {
  background: var(--accent);
  color: var(--fg-on-accent);
  border-color: var(--accent);
}
.pdf-scale {
  font-variant-numeric: tabular-nums;
  font-size: var(--fs-small);
  min-width: 3.5rem;
  text-align: center;
}
.pdf-sep {
  width: 1px;
  align-self: stretch;
  margin: 0 0.2rem;
  background: var(--border);
}
.pdf-find {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: wrap;
  padding: 0.35rem 0.6rem;
  border-bottom: 1px solid var(--border);
  font-size: var(--fs-small);
}
.pdf-find input[type="search"] {
  flex: 1;
  min-width: 10rem;
  max-width: 24rem;
}
.pdf-find label {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
}
.pdf-find-status {
  margin-left: auto;
  opacity: 0.75;
  font-variant-numeric: tabular-nums;
}
.notice {
  margin: 0;
  padding: 0.35rem 0.6rem;
  font-size: var(--fs-small);
  border-bottom: 1px solid var(--border);
  background: var(--bg-raised);
}
.status,
.error {
  margin: 0;
  padding: 0.5rem 0.6rem;
}
.error {
  color: var(--danger);
}
.pdf-progress {
  font-size: var(--fs-small);
  opacity: 0.7;
  font-variant-numeric: tabular-nums;
}
.pdf-body {
  flex: 1;
  min-height: 0;
  display: flex;
}
.pdf-side {
  flex: 0 0 17rem;
  overflow: auto;
  border-right: 1px solid var(--border);
  background: var(--bg-row-alt);
  padding: 0.4rem;
}
.pdf-side ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.pdf-side-empty {
  margin: 0.4rem;
  font-size: var(--fs-small);
  opacity: 0.75;
}
.pdf-link {
  all: unset;
  box-sizing: border-box;
  display: block;
  width: 100%;
  padding: 0.2rem 0.3rem;
  cursor: pointer;
  font-size: var(--fs-small);
}
.pdf-link:hover,
.pdf-link:focus-visible {
  background: var(--accent-soft);
}
.pdf-link:focus-visible {
  outline: 1px solid var(--accent);
}
.pdf-bookmark {
  display: flex;
  align-items: center;
}
.pdf-remove {
  flex: none;
  padding: 0 0.35rem;
}
.pdf-page-area {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  display: flex;
  justify-content: safe center;
  align-items: flex-start;
  padding: 1rem;
  /* Deliberately not a token. A PDF page is white paper, and every
     PDF viewer surrounds it with the same neutral grey in light and
     dark alike -- following the app theme here would make the page
     itself look wrong rather than making the viewer look integrated. */
  background: #525659;
}
.pdf-page-box {
  position: relative;
  flex: none;
  /* The text layer sizes itself from this. */
  --scale-round-x: 1px;
  --scale-round-y: 1px;
}
canvas {
  display: block;
  box-shadow: 0 2px 10px rgb(0 0 0 / 35%);
  background: #fff;
}
.pdf-page-box :deep(.textLayer) {
  /* Above the canvas; the spans inside are transparent text. */
  line-height: 1;
}
.pdf-page-box :deep(mark.pdf-hit) {
  background: rgb(255 213 0 / 45%);
  color: transparent;
  border-radius: 2px;
}
.pdf-page-box :deep(mark.pdf-hit-current) {
  background: rgb(255 120 0 / 55%);
  outline: 2px solid rgb(255 120 0);
}
</style>
