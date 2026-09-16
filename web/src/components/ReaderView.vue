<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { addBookmark, fetchManifest, getAnnotations, getLastReadPositions, setLastReadPosition } from "../reader/api";
import { loadSpineFileInto, type ResolveContext } from "../reader/unserialize";
import { anchorLinkData } from "../reader/virtualLinks";
import { decodePosition, encodePosition } from "../reader/position";
import { flattenToc } from "../reader/toc";
import { DEFAULT_READER_PREFS, fetchProfile, READER_PREFS_PROFILE, type ReaderPrefs } from "../settings/api";
import type { Bookmark, BookManifest } from "../reader/types";

const route = useRoute();

const bookId = computed(() => (route.params.bookId as string) || "");
const fmt = computed(() => ((route.params.fmt as string) || "epub").toLowerCase());

const manifest = ref<BookManifest | null>(null);
const loadError = ref<string | null>(null);
const statusMessage = ref("");
const spineIndex = ref(0);
const showToc = ref(false);
const showBookmarks = ref(false);
const iframeEl = ref<HTMLIFrameElement | null>(null);
const bookmarks = ref<Bookmark[]>([]);
const bookmarkError = ref<string | null>(null);

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
  style.textContent = `html { font-size: ${readerPrefs.value.fontSizePercent}% !important; } body { background: ${bg} !important; color: ${fg} !important; }`;
}

function deviceId(): string {
  const key = "calibre-oxide-device-id";
  let id = localStorage.getItem(key);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(key, id);
  }
  return id;
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
  savePosition(frag);
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

function onKeydown(e: KeyboardEvent) {
  if (e.key === "PageDown" || e.key === "ArrowRight") next();
  else if (e.key === "PageUp" || e.key === "ArrowLeft") prev();
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  void init();
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
watch([bookId, fmt], () => void init());

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
  <div class="reader">
    <header class="toolbar">
      <router-link to="/" class="back">Library</router-link>
      <button @click="showBookmarks = false; showToc = !showToc" :disabled="!manifest">Contents</button>
      <button @click="showToc = false; showBookmarks = !showBookmarks" :disabled="!manifest">Bookmarks ({{ bookmarks.length }})</button>
      <button @click="bookmarkCurrentPage" :disabled="!manifest">Bookmark this page</button>
      <button @click="prev" :disabled="spineIndex <= 0">◀ Prev</button>
      <span class="title">{{ manifest?.metadata?.title ?? "" }}</span>
      <button @click="next" :disabled="!manifest || spineIndex >= manifest.spine.length - 1">Next ▶</button>
    </header>

    <p v-if="!bookId" class="empty">Open a book via <code>/read/&lt;book_id&gt;/&lt;fmt&gt;</code>.</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <p v-else-if="statusMessage" class="status">{{ statusMessage }}</p>
    <p v-if="bookmarkError" class="error">{{ bookmarkError }}</p>

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
