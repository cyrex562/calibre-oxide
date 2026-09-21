<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { addBookmark, addHighlight, fetchBookFileText, fetchManifest, getAnnotations, getLastReadPositions, setLastReadPosition } from "../reader/api";
import { loadSpineFileInto, type ResolveContext } from "../reader/unserialize";
import { anchorLinkData } from "../reader/virtualLinks";
import { decodePosition, deviceId, encodePosition } from "../reader/position";
import { flattenToc } from "../reader/toc";
import { encodeBoundary, rangeFromEncoded } from "../reader/highlightRange";
import { wrapHighlightRange } from "../reader/highlightDom";
import PdfReader from "./PdfReader.vue";
import { extractText, findMatches, totalMatches, type SpineMatches } from "../reader/search";
import { isNoteReference, noteTextFor } from "../reader/footnotes";
import { adjustSpeed, autoScrollPixels, DEFAULT_AUTO_SCROLL_SPEED, detectSwipe, type Point } from "../reader/gestures";
import { clampPageIndex, pageCount, pageForScroll, pagedModeCss, scrollForPage } from "../reader/paged";
import { DEFAULT_KEYMAP, DEFAULT_READER_PREFS, fetchProfile, KEYMAP_PROFILE, READER_PREFS_PROFILE, type KeymapPrefs, type ReaderPrefs } from "../settings/api";
import type { Bookmark, BookManifest, Highlight } from "../reader/types";

const route = useRoute();

const bookId = computed(() => (route.params.bookId as string) || "");
const fmt = computed(() => ((route.params.fmt as string) || "epub").toLowerCase());

// PDFs take a completely different path: there is no manifest, no
// spine and no CFI, so none of the EPUB pipeline below applies. The
// desktop webview is WebKitGTK, which has no built-in PDF viewer, so
// this is rendered by PDF.js rather than handed to an `<embed>`.
const isPdf = computed(() => fmt.value === "pdf");

const manifest = ref<BookManifest | null>(null);
const loadError = ref<string | null>(null);
const statusMessage = ref("");
const spineIndex = ref(0);
const showToc = ref(false);
const showBookmarks = ref(false);
const iframeEl = ref<HTMLIFrameElement | null>(null);
const bookmarks = ref<Bookmark[]>([]);
const bookmarkError = ref<string | null>(null);
const highlights = ref<Highlight[]>([]);
const highlightError = ref<string | null>(null);

const tocEntries = computed(() => (manifest.value ? flattenToc(manifest.value.toc) : []));

// Real reading preferences (issue #721) -- fetched once on mount,
// re-applied to the sandboxed content iframe after every spine load
// (a fresh document each time, so the injected <style> doesn't
// survive navigation on its own).
const readerPrefs = ref<ReaderPrefs>({ ...DEFAULT_READER_PREFS });

async function loadReaderPrefs() {
  try {
    const prefs = await fetchProfile<ReaderPrefs>(READER_PREFS_PROFILE);
    if (prefs) readerPrefs.value = prefs;
  } catch (e) {
    console.error("failed to load reading preferences", e);
  }
}

// Real keyboard shortcut customization (#752) -- loaded once on
// mount, same pattern as readerPrefs above.
const keymap = ref<KeymapPrefs>({ ...DEFAULT_KEYMAP });

async function loadKeymap() {
  try {
    const prefs = await fetchProfile<KeymapPrefs>(KEYMAP_PROFILE);
    if (prefs) keymap.value = { ...DEFAULT_KEYMAP, ...prefs };
  } catch (e) {
    console.error("failed to load keyboard shortcuts", e);
  }
}

const THEME_COLORS: Record<ReaderPrefs["theme"], { bg: string; fg: string }> = {
  light: { bg: "#ffffff", fg: "#111111" },
  dark: { bg: "#181818", fg: "#e8e8e8" },
  sepia: { bg: "#f4ecd8", fg: "#3b3226" },
};

const READER_PREFS_STYLE_ID = "calibre-oxide-reading-prefs";

function applyReaderPrefs() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;
  const { bg, fg } = THEME_COLORS[readerPrefs.value.theme];
  let style = doc.getElementById(READER_PREFS_STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = doc.createElement("style");
    style.id = READER_PREFS_STYLE_ID;
    doc.head?.appendChild(style);
  }
  style.textContent = `html { font-size: ${readerPrefs.value.fontSizePercent}% !important; } body { background: ${bg} !important; color: ${fg} !important; } mark.cx-highlight { background: #ffe066 !important; color: #111 !important; }`;
}

async function pollManifest(): Promise<BookManifest> {
  for (let attempt = 0; attempt < 300; attempt++) {
    const m = await fetchManifest(bookId.value, fmt.value);
    if (!m.job_status) return m;
    if (m.job_status === "failed") {
      throw new Error(`render job failed: ${m.traceback ?? "unknown error"}`);
    }
    statusMessage.value = `Preparing book for reading… (${m.job_status})`;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("timed out waiting for the book to finish rendering");
}

function resolveContextFor(m: BookManifest): ResolveContext {
  return {
    bookId: bookId.value,
    fmt: fmt.value,
    size: m.book_hash.size,
    mtime: m.book_hash.mtime,
    linkUid: m.link_uid,
    textCache: new Map(),
  };
}

async function loadSpine(index: number, frag = "") {
  const m = manifest.value;
  if (!m || !iframeEl.value?.contentDocument) return;
  const name = m.spine[index];
  if (!name) return;
  spineIndex.value = index;
  await loadSpineFileInto(iframeEl.value.contentDocument, resolveContextFor(m), name);
  applyReaderPrefs();
  applyPagedMode();
  if (frag) {
    iframeEl.value.contentDocument.getElementById(frag)?.scrollIntoView();
  }
  installAnchorHandler(m);
  installSelectionHandler();
  installTouchHandler();
  renderHighlights();
  savePosition(frag);
}

// Real text-highlight rendering (issue #731) -- every highlight whose
// own encoded boundaries belong to the spine file just loaded (a
// highlight never spans more than one spine file in this reader, see
// reader/highlightRange.ts's own doc) is re-wrapped in a real <mark>
// each time that file loads, since loadSpineFileInto rebuilds the
// iframe's document from scratch on every navigation.
function renderHighlights() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;
  for (const h of highlights.value) {
    const range = rangeFromEncoded(doc, spineIndex.value, h.start_cfi, h.end_cfi);
    if (range) wrapHighlightRange(range, "cx-highlight", { uuid: h.uuid });
  }
}

function installAnchorHandler(m: BookManifest) {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;
  doc.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const anchor = target?.closest("a,area");
    if (!anchor) return;
    const data = anchorLinkData(anchor, m.link_uid);
    if (!data) return;
    event.preventDefault();

    // A footnote pops over the text rather than navigating: following
    // the link means losing your place and having to come back.
    if (!data.missing && data.frag && isNoteReference(anchor.getAttribute("epub:type"), anchor.getAttribute("role"))) {
      void showFootnote(m, data.name, data.frag, anchor);
      return;
    }
    if (data.missing) {
      statusMessage.value = `That link points to a resource that isn't part of this book.`;
      return;
    }
    const idx = m.spine.indexOf(data.name);
    if (idx === -1) {
      statusMessage.value = `That link points outside the book's spine (${data.name}) -- not yet supported by this reader slice.`;
      return;
    }
    void loadSpine(idx, data.frag);
  });
}

const HIGHLIGHT_POPOVER_ID = "calibre-oxide-highlight-popover";

// Real text-selection -> highlight creation flow (issue #731). The
// popover button is real content injected directly into the iframe's
// own document (not a separate overlay positioned across the frame
// boundary) so it naturally lives in the same coordinate space as the
// selection it points at, with no cross-frame rect translation
// needed. `loadSpineFileInto`'s own `document.open()/write()/close()`
// (see unserialize.ts) resets the whole document -- including every
// previously attached listener -- on each navigation, so re-attaching
// here on every `loadSpine` call (matching `installAnchorHandler`'s
// own identical pattern) is the correct, not redundant, thing to do.
function installSelectionHandler() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;

  // A bar rather than a lone button (#2.4): highlighting was the only
  // thing a selection could do, and copying a quotation is at least
  // as common.
  const popover = doc.createElement("div");
  popover.id = HIGHLIGHT_POPOVER_ID;
  popover.style.cssText = "position:absolute;z-index:1000;display:none;gap:1px;border-radius:4px;overflow:hidden;box-shadow:0 2px 8px rgba(0,0,0,0.3);font:14px sans-serif;";

  const makeButton = (label: string, onClick: () => void) => {
    const button = doc.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.style.cssText = "padding:0.3em 0.7em;border:none;background:#2a6df4;color:#fff;cursor:pointer;font:inherit;";
    // Without this, the mousedown on the button itself would collapse
    // the very selection it is meant to act on before the click
    // handler ever runs.
    button.addEventListener("mousedown", (e) => e.preventDefault());
    button.addEventListener("click", onClick);
    popover.appendChild(button);
    return button;
  };

  makeButton("Highlight", () => void createHighlightFromSelection());
  makeButton("Copy", () => void copySelection(doc, popover));
  doc.body.appendChild(popover);

  doc.addEventListener("mouseup", () => {
    const sel = doc.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !sel.toString().trim()) {
      popover.style.display = "none";
      return;
    }
    const rect = sel.getRangeAt(0).getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) {
      popover.style.display = "none";
      return;
    }
    const view = doc.defaultView;
    popover.style.left = `${rect.left + (view?.scrollX ?? 0)}px`;
    popover.style.top = `${rect.top + (view?.scrollY ?? 0) - 32}px`;
    popover.style.display = "flex";
  });
}

/**
 * Copies the selection (#2.4).
 *
 * `navigator.clipboard` is unavailable in some webview contexts and
 * requires a secure origin, so a failure falls back to the older
 * `execCommand` path rather than silently doing nothing -- the user
 * pressed a button labelled Copy.
 */
async function copySelection(doc: Document, popover: HTMLElement) {
  const text = doc.getSelection()?.toString() ?? "";
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    try {
      doc.execCommand("copy");
    } catch {
      statusMessage.value = "Could not copy the selection.";
      return;
    }
  }
  popover.style.display = "none";
  statusMessage.value = "Copied.";
}

async function createHighlightFromSelection() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;
  const sel = doc.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  const text = range.toString();
  if (!text.trim()) return;

  const startCfi = encodeBoundary(doc.body, spineIndex.value, range.startContainer, range.startOffset);
  const endCfi = encodeBoundary(doc.body, spineIndex.value, range.endContainer, range.endOffset);
  const popover = doc.getElementById(HIGHLIGHT_POPOVER_ID);
  if (popover) popover.style.display = "none";
  if (!startCfi || !endCfi) {
    highlightError.value = "Could not anchor this selection to a real position -- try selecting within a single paragraph.";
    return;
  }
  highlightError.value = null;
  try {
    const highlight = await addHighlight(bookId.value, fmt.value, startCfi, endCfi, text);
    highlights.value = [...highlights.value, highlight];
    wrapHighlightRange(range, "cx-highlight", { uuid: highlight.uuid });
    sel.removeAllRanges();
  } catch (e) {
    highlightError.value = e instanceof Error ? e.message : String(e);
  }
}

let saveTimer: ReturnType<typeof setTimeout> | null = null;
function savePosition(frag: string) {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    void setLastReadPosition(bookId.value, fmt.value, deviceId(), encodePosition({ spineIndex: spineIndex.value, frag }), spineIndex.value / Math.max(1, (manifest.value?.spine.length ?? 1) - 1));
  }, 300);
}

async function init() {
  if (!bookId.value) return;
  loadError.value = null;
  try {
    statusMessage.value = "Loading…";
    await loadReaderPrefs();
    await loadKeymap();
    const m = await pollManifest();
    manifest.value = m;
    statusMessage.value = "";

    // Not `m.annotations_map`: `render_endpoints.rs` only populates
    // that field when the request is authenticated
    // (`is_authenticated` gate), and this app's own spawned
    // `calibre_srv` runs with auth disabled by default -- the
    // manifest's copy would silently always be empty in real use.
    // `/book-get-annotations` has no such gate (its `effective_user`
    // falls back to the anonymous user id), so it's the real source
    // of truth here regardless of auth state.
    try {
      const map = await getAnnotations(bookId.value, fmt.value);
      bookmarks.value = map.bookmark ?? [];
      highlights.value = map.highlight ?? [];
    } catch (e) {
      console.error("failed to load bookmarks", e);
    }

    const positions = await getLastReadPositions(bookId.value, fmt.value);
    const device = deviceId();
    const mine = positions.find((p) => p.device === device) ?? positions[0];
    const pos = decodePosition(mine?.cfi);
    await loadSpine(pos?.spineIndex ?? 0, pos?.frag ?? "");
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : String(e);
  }
}

function next() {
  if (manifest.value && spineIndex.value < manifest.value.spine.length - 1) {
    void loadSpine(spineIndex.value + 1);
  }
}
function prev() {
  if (spineIndex.value > 0) {
    void loadSpine(spineIndex.value - 1);
  }
}

// Real "Read aloud" TTS (#756) -- built on the new POST /tts/synthesize
// route (crates/calibre_srv/src/tts.rs), which itself reuses
// calibre_ebooks::tts::batch::text_to_raw_audio_data (already wraps
// stream::Piper for exactly this "synthesize on demand" shape). Real,
// disclosed narrowing: this synthesizes the current spine file's whole
// visible text in one request/one WAV rather than per-sentence/per-
// paragraph chunked streaming with sentence highlighting -- see
// tts.rs's own doc.
const readAloudActive = ref(false);
const readAloudLoading = ref(false);
const readAloudError = ref<string | null>(null);
const audioEl = ref<HTMLAudioElement | null>(null);
let currentAudioUrl: string | null = null;

function releaseAudioUrl() {
  if (currentAudioUrl) {
    URL.revokeObjectURL(currentAudioUrl);
    currentAudioUrl = null;
  }
}

async function synthesizeAndPlayCurrentPage() {
  const doc = iframeEl.value?.contentDocument;
  const text = doc?.body?.innerText?.trim() ?? "";
  if (!text) {
    // Nothing to read on this (near-empty) page -- auto-advance if
    // there's a next one, matching real upstream's own auto-advance
    // once a page finishes; otherwise stop.
    await advanceReadAloud();
    return;
  }
  readAloudLoading.value = true;
  readAloudError.value = null;
  try {
    const resp = await fetch("/tts/synthesize", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ text }) });
    if (!resp.ok) throw new Error((await resp.text()) || `${resp.status} ${resp.statusText}`);
    const blob = await resp.blob();
    releaseAudioUrl();
    currentAudioUrl = URL.createObjectURL(blob);
    await nextTick();
    if (audioEl.value) {
      audioEl.value.src = currentAudioUrl;
      await audioEl.value.play();
    }
  } catch (e) {
    readAloudError.value = e instanceof Error ? e.message : String(e);
    readAloudActive.value = false;
  } finally {
    readAloudLoading.value = false;
  }
}

async function advanceReadAloud() {
  if (!manifest.value || spineIndex.value >= manifest.value.spine.length - 1) {
    readAloudActive.value = false;
    return;
  }
  await loadSpine(spineIndex.value + 1);
  if (readAloudActive.value) await synthesizeAndPlayCurrentPage();
}

function onReadAloudEnded() {
  if (readAloudActive.value) void advanceReadAloud();
}

function toggleReadAloud() {
  readAloudActive.value = !readAloudActive.value;
  if (readAloudActive.value) {
    void synthesizeAndPlayCurrentPage();
  } else {
    audioEl.value?.pause();
    releaseAudioUrl();
  }
}

onBeforeUnmount(() => releaseAudioUrl());

function onKeydown(e: KeyboardEvent) {
  if (e.key === keymap.value.readerNext) nextPage();
  else if (e.key === keymap.value.readerPrev) prevPage();
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("resize", onReaderResize);
  if (!isPdf.value) void init();
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("resize", onReaderResize);
  stopAutoScroll();
});
watch([bookId, fmt], () => {
  if (!isPdf.value) void init();
});

function goToTocEntry(dest: string | null, frag: string | null) {
  const m = manifest.value;
  if (!m) return;
  const idx = dest ? m.spine.indexOf(dest) : spineIndex.value;
  if (idx === -1) return;
  showToc.value = false;
  void loadSpine(idx, frag ?? "");
}

async function bookmarkCurrentPage() {
  const m = manifest.value;
  if (!m) return;
  const suggested = tocEntries.value.find((e) => m.spine.indexOf(e.dest ?? "") === spineIndex.value)?.title ?? `Page ${spineIndex.value + 1}`;
  const title = prompt("Bookmark title:", suggested);
  if (!title) return;
  bookmarkError.value = null;
  try {
    const bookmark = await addBookmark(bookId.value, fmt.value, title, encodePosition({ spineIndex: spineIndex.value, frag: "" }));
    // Re-bookmarking under the same title replaces it server-side
    // (annotations.rs's own title-keyed merge) -- mirror that locally
    // rather than appending a duplicate entry.
    bookmarks.value = [...bookmarks.value.filter((b) => b.title !== title), bookmark];
  } catch (e) {
    bookmarkError.value = e instanceof Error ? e.message : String(e);
  }
}

function goToBookmark(bookmark: Bookmark) {
  const pos = decodePosition(bookmark.pos);
  if (!pos) return;
  showBookmarks.value = false;
  void loadSpine(pos.spineIndex, pos.frag);
}

// ---------------------------------------------------------------
// In-book search (#2.2)
// ---------------------------------------------------------------
//
// A book could be read but not searched, which for reference
// material is most of the reason to open one.
//
// Every spine file is fetched and searched, so this is a real
// whole-book search rather than a search of whatever happens to be on
// screen. Files are fetched sequentially and the results stream in,
// because a long book is many requests and a user should see early
// hits rather than a spinner.

const searchOpen = ref(false);
const searchQuery = ref("");
const searchWholeWord = ref(false);
const searchCaseSensitive = ref(false);
const searchResults = ref<SpineMatches[]>([]);
const searching = ref(false);
const searchProgress = ref(0);
const searchError = ref<string | null>(null);

/** Bumped per search so a superseded run stops writing results. */
let searchRun = 0;

const searchTotal = computed(() => totalMatches(searchResults.value));

async function runBookSearch() {
  const m = manifest.value;
  const query = searchQuery.value.trim();
  if (!m || !query) return;

  const run = ++searchRun;
  searching.value = true;
  searchError.value = null;
  searchResults.value = [];
  searchProgress.value = 0;

  const ctx = resolveContextFor(m);
  const options = { wholeWord: searchWholeWord.value, caseSensitive: searchCaseSensitive.value };

  try {
    for (let i = 0; i < m.spine.length; i += 1) {
      if (run !== searchRun) return; // a newer search started
      const name = m.spine[i];
      try {
        const raw = await fetchBookFileText(ctx.bookId, ctx.fmt, ctx.size, ctx.mtime, name);
        const parsed = JSON.parse(raw) as { tree: Parameters<typeof extractText>[0] };
        const matches = findMatches(extractText(parsed.tree), query, options);
        if (run !== searchRun) return;
        if (matches.length > 0) searchResults.value = [...searchResults.value, { spineIndex: i, name, matches }];
      } catch {
        // One unreadable spine file must not abandon the rest of the
        // book -- a search that silently stops halfway is worse than
        // one that reports slightly fewer hits.
      }
      searchProgress.value = i + 1;
    }
  } catch (e) {
    if (run === searchRun) searchError.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (run === searchRun) searching.value = false;
  }
}

async function goToSearchHit(result: SpineMatches) {
  showToc.value = false;
  showBookmarks.value = false;
  await loadSpine(result.spineIndex);
}

// ---------------------------------------------------------------
// Popup footnotes (#2.6)
// ---------------------------------------------------------------
//
// Note detection and text extraction live in reader/footnotes.ts,
// verified against a real EPUB served by this stack: `epub:type`
// survives serialization verbatim, so the marker really is there to
// match on.

const FOOTNOTE_POPOVER_ID = "calibre-oxide-footnote-popover";

async function showFootnote(m: BookManifest, name: string, frag: string, anchor: Element) {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;

  let text: string | null = null;
  try {
    const ctx = resolveContextFor(m);
    const raw = await fetchBookFileText(ctx.bookId, ctx.fmt, ctx.size, ctx.mtime, name);
    const parsed = JSON.parse(raw) as { tree: Parameters<typeof noteTextFor>[0] };
    text = noteTextFor(parsed.tree, frag);
  } catch {
    text = null;
  }

  // Fall back to ordinary navigation rather than showing an empty
  // bubble: an unresolvable note is still a real link somewhere.
  if (!text) {
    const idx = m.spine.indexOf(name);
    if (idx !== -1) void loadSpine(idx, frag);
    return;
  }

  doc.getElementById(FOOTNOTE_POPOVER_ID)?.remove();

  const popover = doc.createElement("div");
  popover.id = FOOTNOTE_POPOVER_ID;
  popover.textContent = text;
  popover.style.cssText = [
    "position:absolute",
    "z-index:1001",
    "max-width:min(34em, 80vw)",
    "max-height:40vh",
    "overflow-y:auto",
    "padding:0.7em 0.9em",
    "border-radius:6px",
    "border:1px solid rgba(0,0,0,0.2)",
    "background:#fffef8",
    "color:#111",
    "box-shadow:0 4px 18px rgba(0,0,0,0.25)",
    "font:inherit",
    "line-height:1.45",
  ].join(";");

  const rect = anchor.getBoundingClientRect();
  const view = doc.defaultView;
  popover.style.left = `${Math.max(8, rect.left + (view?.scrollX ?? 0) - 40)}px`;
  popover.style.top = `${rect.bottom + (view?.scrollY ?? 0) + 6}px`;
  doc.body.appendChild(popover);

  // Dismissed by clicking anywhere else, which is what a reader
  // expects and needs no close button competing with the note text.
  const dismiss = (e: Event) => {
    if (popover.contains(e.target as Node)) return;
    popover.remove();
    doc.removeEventListener("click", dismiss, true);
  };
  // Deferred so the click that opened it does not immediately close it.
  setTimeout(() => doc.addEventListener("click", dismiss, true), 0);
}

// ---------------------------------------------------------------
// Touch gestures, auto-scroll and printing (#2.7 / #2.8)
// ---------------------------------------------------------------
//
// The reader was mouse-and-keyboard only: on a tablet or a
// touchscreen laptop there was no way to turn a page at all. The
// geometry lives in reader/gestures.ts, where its thresholds can be
// stated and tested -- every one of them is wrong in a specific,
// reproducible way if guessed.

function installTouchHandler() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc) return;

  let start: Point | null = null;

  doc.addEventListener(
    "touchstart",
    (event) => {
      const t = event.touches[0];
      start = t ? { x: t.clientX, y: t.clientY, t: event.timeStamp } : null;
    },
    { passive: true },
  );

  doc.addEventListener(
    "touchend",
    (event) => {
      const t = event.changedTouches[0];
      if (!start || !t) return;
      const swipe = detectSwipe(start, { x: t.clientX, y: t.clientY, t: event.timeStamp });
      start = null;
      // Only horizontal swipes turn pages: vertical ones are the
      // reader scrolling, which the browser already handles.
      if (swipe === "left") nextPage();
      else if (swipe === "right") prevPage();
    },
    { passive: true },
  );
}

const autoScrolling = ref(false);
const autoScrollSpeed = ref(DEFAULT_AUTO_SCROLL_SPEED);
let autoScrollFrame: number | null = null;
let autoScrollLast = 0;
// Sub-pixel remainder: rounding every tick to a whole pixel would
// make the slowest speeds round to zero and never move.
let autoScrollRemainder = 0;

function stepAutoScroll(now: number) {
  const view = iframeEl.value?.contentWindow;
  const doc = iframeEl.value?.contentDocument;
  if (!autoScrolling.value || !view || !doc) return;

  const elapsed = autoScrollLast ? now - autoScrollLast : 0;
  autoScrollLast = now;

  autoScrollRemainder += autoScrollPixels(autoScrollSpeed.value, elapsed);
  const whole = Math.floor(autoScrollRemainder);
  if (whole > 0) {
    autoScrollRemainder -= whole;
    const before = view.scrollY;
    view.scrollBy(0, whole);
    // At the bottom of a section, move to the next one rather than
    // stalling against the end of the document.
    if (view.scrollY === before) {
      const m = manifest.value;
      if (m && spineIndex.value < m.spine.length - 1) next();
      else stopAutoScroll();
    }
  }
  autoScrollFrame = view.requestAnimationFrame(stepAutoScroll);
}

function startAutoScroll() {
  const view = iframeEl.value?.contentWindow;
  if (!view) return;
  autoScrolling.value = true;
  autoScrollLast = 0;
  autoScrollRemainder = 0;
  autoScrollFrame = view.requestAnimationFrame(stepAutoScroll);
}

function stopAutoScroll() {
  autoScrolling.value = false;
  const view = iframeEl.value?.contentWindow;
  if (autoScrollFrame !== null && view) view.cancelAnimationFrame(autoScrollFrame);
  autoScrollFrame = null;
}

function toggleAutoScroll() {
  if (autoScrolling.value) stopAutoScroll();
  else startAutoScroll();
}

function changeAutoScrollSpeed(delta: number) {
  autoScrollSpeed.value = adjustSpeed(autoScrollSpeed.value, delta);
}

/**
 * Prints the section on screen (#2.8).
 *
 * The iframe prints itself, so the book's own stylesheet applies and
 * the reader's chrome does not. Printing the *whole* book would mean
 * assembling every spine file into one document first, which is a
 * different feature; upstream generates a PDF for that.
 */
function printCurrentSection() {
  const view = iframeEl.value?.contentWindow;
  if (!view) return;
  // Auto-scroll fighting the print dialog moves the page under the
  // user while they are looking at it.
  stopAutoScroll();
  view.focus();
  view.print();
}

// ---------------------------------------------------------------
// Paged mode (#2.3)
// ---------------------------------------------------------------
//
// The reader only ever scrolled. Paged mode is what most people mean
// by reading an ebook: fixed pages that turn.
//
// The content is laid out with CSS multi-column at exactly the
// viewport width, so it flows into side-by-side columns and turning a
// page is a horizontal scroll -- no re-layout, no measuring text, and
// the book's own stylesheet still applies. The arithmetic lives in
// reader/paged.ts, where its off-by-ones can be stated and tested.

const PAGED_STYLE_ID = "calibre-oxide-paged-mode";

const pagedMode = ref(false);
const currentPage = ref(0);
const pagesInSection = ref(1);

function pagedViewport(): { width: number; height: number } | null {
  const view = iframeEl.value?.contentWindow;
  if (!view) return null;
  return { width: view.innerWidth, height: view.innerHeight };
}

/** Installs or removes the multi-column layout. */
function applyPagedMode() {
  const doc = iframeEl.value?.contentDocument;
  if (!doc?.head) return;

  let style = doc.getElementById(PAGED_STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = doc.createElement("style");
    style.id = PAGED_STYLE_ID;
    doc.head.appendChild(style);
  }

  if (!pagedMode.value) {
    style.textContent = "";
    pagesInSection.value = 1;
    currentPage.value = 0;
    return;
  }

  const vp = pagedViewport();
  if (!vp) return;
  style.textContent = pagedModeCss(vp.width, vp.height);
  measurePages();
}

function measurePages() {
  const doc = iframeEl.value?.contentDocument;
  const vp = pagedViewport();
  if (!doc?.documentElement || !vp) return;
  pagesInSection.value = pageCount(doc.documentElement.scrollWidth, vp.width);
  currentPage.value = clampPageIndex(pageForScroll(doc.documentElement.scrollLeft, vp.width), pagesInSection.value);
}

function goToPage(page: number) {
  const doc = iframeEl.value?.contentDocument;
  const vp = pagedViewport();
  if (!doc?.documentElement || !vp) return;
  const target = clampPageIndex(page, pagesInSection.value);
  doc.documentElement.scrollLeft = scrollForPage(target, vp.width);
  currentPage.value = target;
}

/**
 * Page forward, crossing into the next section at the end.
 *
 * In scrolling mode "next" has always meant the next section; in
 * paged mode it means the next *page*, and only falls through to the
 * next section once there are no pages left.
 */
function nextPage() {
  if (!pagedMode.value) {
    next();
    return;
  }
  if (currentPage.value < pagesInSection.value - 1) goToPage(currentPage.value + 1);
  else next();
}

function prevPage() {
  if (!pagedMode.value) {
    prev();
    return;
  }
  if (currentPage.value > 0) goToPage(currentPage.value - 1);
  else prev();
}

function togglePagedMode() {
  pagedMode.value = !pagedMode.value;
  applyPagedMode();
  // Auto-scroll scrolls vertically, which a paged layout has none of.
  if (pagedMode.value) stopAutoScroll();
}

// A resize changes the column width, so the layout and the page count
// both have to be recomputed -- otherwise the reader is left showing
// a position that no longer exists.
function onReaderResize() {
  if (pagedMode.value) applyPagedMode();
}
</script>

<template>
  <!--
    A PDF shares only the "Library" link with the EPUB reader: no
    contents, bookmarks, spine navigation or read-aloud apply to it,
    so it gets its own view rather than a toolbar full of disabled
    buttons.
  -->
  <div v-if="isPdf" class="reader">
    <header class="toolbar">
      <router-link to="/" class="back">Library</router-link>
      <span class="title">PDF</span>
    </header>
    <PdfReader :book-id="bookId" />
  </div>

  <div v-else class="reader">
    <header class="toolbar">
      <router-link to="/" class="back">Library</router-link>
      <button @click="showBookmarks = false; showToc = !showToc" :disabled="!manifest">Contents</button>
      <button @click="showToc = false; showBookmarks = !showBookmarks" :disabled="!manifest">Bookmarks ({{ bookmarks.length }})</button>
      <button @click="showToc = false; showBookmarks = false; searchOpen = !searchOpen" :disabled="!manifest" :class="{ active: searchOpen }">Search</button>
      <button @click="bookmarkCurrentPage" :disabled="!manifest">Bookmark this page</button>
      <button @click="prevPage" :disabled="!pagedMode && spineIndex <= 0">◀ Prev</button>
      <span class="title">{{ manifest?.metadata?.title ?? "" }}</span>
      <button @click="nextPage" :disabled="!manifest || (!pagedMode && spineIndex >= manifest.spine.length - 1)">Next ▶</button>
      <button @click="togglePagedMode" :disabled="!manifest" :class="{ active: pagedMode }" title="Switch between paged and scrolling">{{ pagedMode ? "Paged" : "Scrolling" }}</button>
      <span v-if="pagedMode" class="page-indicator">{{ currentPage + 1 }} / {{ pagesInSection }}</span>
      <button @click="printCurrentSection" :disabled="!manifest" title="Print the section on screen">Print</button>
      <span class="autoscroll" :class="{ active: autoScrolling }">
        <button @click="toggleAutoScroll" :disabled="!manifest">{{ autoScrolling ? "⏸ Auto-scroll" : "▶ Auto-scroll" }}</button>
        <template v-if="autoScrolling">
          <button @click="changeAutoScrollSpeed(-1)" title="Slower">−</button>
          <span class="autoscroll-speed">{{ autoScrollSpeed }}×</span>
          <button @click="changeAutoScrollSpeed(1)" title="Faster">+</button>
        </template>
      </span>
      <button @click="toggleReadAloud" :disabled="!manifest" :class="{ active: readAloudActive }">
        {{ readAloudLoading ? "Synthesizing…" : readAloudActive ? "⏹ Stop reading" : "🔊 Read aloud" }}
      </button>
    </header>

    <p v-if="!bookId" class="empty">Open a book via <code>/read/&lt;book_id&gt;/&lt;fmt&gt;</code>.</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <p v-else-if="statusMessage" class="status">{{ statusMessage }}</p>
    <p v-if="bookmarkError" class="error">{{ bookmarkError }}</p>
    <p v-if="highlightError" class="error">{{ highlightError }}</p>
    <p v-if="readAloudError" class="error">{{ readAloudError }}</p>
    <audio v-show="readAloudActive" ref="audioEl" controls @ended="onReadAloudEnded" class="read-aloud-player" />

    <aside v-if="searchOpen" class="book-search">
      <form class="book-search-form" @submit.prevent="runBookSearch">
        <input v-model="searchQuery" type="search" placeholder="Search this book…" autofocus />
        <button type="submit" :disabled="searching || !searchQuery.trim()">{{ searching ? "Searching…" : "Search" }}</button>
      </form>
      <div class="book-search-options">
        <label><input v-model="searchWholeWord" type="checkbox" /> Whole word</label>
        <label><input v-model="searchCaseSensitive" type="checkbox" /> Match case</label>
        <span v-if="manifest && searching" class="book-search-progress">{{ searchProgress }} / {{ manifest.spine.length }}</span>
      </div>

      <p v-if="searchError" class="error">{{ searchError }}</p>
      <p v-else-if="!searching && searchQuery.trim() && searchTotal === 0 && searchProgress > 0" class="status">No matches.</p>
      <p v-else-if="searchTotal > 0" class="status">{{ searchTotal }} match(es) in {{ searchResults.length }} section(s)</p>

      <ul class="book-search-results">
        <li v-for="result in searchResults" :key="result.spineIndex">
          <button type="button" class="book-search-section" @click="goToSearchHit(result)">
            Section {{ result.spineIndex + 1 }} — {{ result.matches.length }} match(es)
          </button>
          <ul class="book-search-hits">
            <li v-for="(m, i) in result.matches.slice(0, 5)" :key="i" @click="goToSearchHit(result)">
              <span>{{ m.context.slice(0, m.contextOffset) }}</span><mark>{{ m.text }}</mark><span>{{ m.context.slice(m.contextOffset + m.text.length) }}</span>
            </li>
            <li v-if="result.matches.length > 5" class="book-search-more">+{{ result.matches.length - 5 }} more in this section</li>
          </ul>
        </li>
      </ul>
    </aside>

    <nav v-if="showToc" class="toc">
      <ul>
        <li v-for="(entry, i) in tocEntries" :key="i" :style="{ paddingLeft: `${entry.depth}em` }">
          <a href="#" @click.prevent="goToTocEntry(entry.dest, entry.frag)">{{ entry.title }}</a>
        </li>
      </ul>
    </nav>

    <nav v-if="showBookmarks" class="toc">
      <p v-if="bookmarks.length === 0" class="empty">No bookmarks yet.</p>
      <ul v-else>
        <li v-for="b in bookmarks" :key="b.title">
          <a href="#" @click.prevent="goToBookmark(b)">{{ b.title }}</a>
        </li>
      </ul>
    </nav>

    <iframe ref="iframeEl" class="content" sandbox="allow-same-origin" title="book content" />
  </div>
</template>

<style scoped>
.reader {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.5em;
  border-bottom: 1px solid #ddd;
}
.toolbar .title {
  flex: 1;
  text-align: center;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.toolbar button.active {
  background: #2a6fbf;
  color: #fff;
  border-color: #2a6fbf;
}
.read-aloud-player {
  width: 100%;
  padding: 0 0.5em;
}
.content {
  flex: 1;
  border: none;
  width: 100%;
}
.toc {
  position: absolute;
  top: 3em;
  left: 0;
  bottom: 0;
  width: 280px;
  overflow: auto;
  background: #fafafa;
  border-right: 1px solid #ddd;
  padding: 0.5em;
}
.toc ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.toc a {
  display: block;
  padding: 0.25em 0;
  text-decoration: none;
  color: inherit;
}
.empty,
.error,
.status {
  padding: 1em;
}
.error {
  color: #b00020;
}
/* In-book search (#2.2). */
.book-search {
  position: absolute;
  top: 3rem;
  left: 0;
  right: 0;
  max-height: 60vh;
  overflow-y: auto;
  background: #fff;
  border-bottom: 1px solid #ccc;
  padding: 0.6rem 0.8rem;
  z-index: 10;
}
.book-search-form {
  display: flex;
  gap: 0.4rem;
}
.book-search-form input {
  flex: 1;
}
.book-search-options {
  display: flex;
  gap: 1rem;
  align-items: center;
  font-size: 0.82rem;
  margin: 0.35rem 0;
}
.book-search-progress {
  margin-left: auto;
  opacity: 0.6;
  font-variant-numeric: tabular-nums;
}
.book-search-results {
  list-style: none;
  margin: 0;
  padding: 0;
}
.book-search-section {
  all: unset;
  cursor: pointer;
  font-weight: 600;
  font-size: 0.85rem;
  display: block;
  margin-top: 0.4rem;
}
.book-search-section:hover {
  text-decoration: underline;
}
.book-search-hits {
  list-style: none;
  margin: 0.2rem 0 0;
  padding: 0 0 0 0.8rem;
  font-size: 0.83rem;
}
.book-search-hits li {
  cursor: pointer;
  padding: 0.12rem 0;
  border-left: 2px solid #e0e0e0;
  padding-left: 0.5rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.book-search-hits li:hover {
  background: #f3f6ff;
}
.book-search-more {
  opacity: 0.6;
  cursor: default !important;
}
@media (prefers-color-scheme: dark) {
  .book-search {
    background: #1a1d23;
    border-bottom-color: #3a3d44;
  }
  .book-search-hits li {
    border-left-color: #3a3d44;
  }
  .book-search-hits li:hover {
    background: #252b36;
  }
}

/* Auto-scroll controls (#2.8). */
.autoscroll {
  display: inline-flex;
  align-items: center;
  gap: 0.2rem;
}
.autoscroll.active {
  outline: 1px solid currentColor;
  border-radius: 4px;
  padding: 0 0.15rem;
}
.autoscroll-speed {
  font-variant-numeric: tabular-nums;
  font-size: 0.85rem;
  min-width: 2.5rem;
  text-align: center;
}

/* Paged mode (#2.3). */
.page-indicator {
  font-variant-numeric: tabular-nums;
  font-size: 0.85rem;
  opacity: 0.7;
  min-width: 3.5rem;
  text-align: center;
}

</style>
