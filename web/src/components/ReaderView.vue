<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { addBookmark, addHighlight, fetchManifest, getAnnotations, getLastReadPositions, setLastReadPosition } from "../reader/api";
import { loadSpineFileInto, type ResolveContext } from "../reader/unserialize";
import { anchorLinkData } from "../reader/virtualLinks";
import { decodePosition, deviceId, encodePosition } from "../reader/position";
import { flattenToc } from "../reader/toc";
import { encodeBoundary, rangeFromEncoded } from "../reader/highlightRange";
import { wrapHighlightRange } from "../reader/highlightDom";
import PdfReader from "./PdfReader.vue";
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
  if (frag) {
    iframeEl.value.contentDocument.getElementById(frag)?.scrollIntoView();
  }
  installAnchorHandler(m);
  installSelectionHandler();
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

  const popover = doc.createElement("button");
  popover.id = HIGHLIGHT_POPOVER_ID;
  popover.type = "button";
  popover.textContent = "Highlight";
  popover.style.cssText = "position:absolute;z-index:1000;display:none;padding:0.3em 0.6em;border-radius:4px;border:none;background:#2a6df4;color:#fff;font:14px sans-serif;cursor:pointer;";
  doc.body.appendChild(popover);
  // Without this, the mousedown on the button itself would collapse
  // the very selection it's meant to act on before the click handler
  // below ever runs.
  popover.addEventListener("mousedown", (e) => e.preventDefault());
  popover.addEventListener("click", () => void createHighlightFromSelection());

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
    popover.style.display = "block";
  });
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
  if (e.key === keymap.value.readerNext) next();
  else if (e.key === keymap.value.readerPrev) prev();
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  if (!isPdf.value) void init();
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
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
      <button @click="bookmarkCurrentPage" :disabled="!manifest">Bookmark this page</button>
      <button @click="prev" :disabled="spineIndex <= 0">◀ Prev</button>
      <span class="title">{{ manifest?.metadata?.title ?? "" }}</span>
      <button @click="next" :disabled="!manifest || spineIndex >= manifest.spine.length - 1">Next ▶</button>
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
</style>
