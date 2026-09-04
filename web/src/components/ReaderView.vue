<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { fetchManifest, getLastReadPositions, setLastReadPosition } from "../reader/api";
import { loadSpineFileInto, type ResolveContext } from "../reader/unserialize";
import { anchorLinkData } from "../reader/virtualLinks";
import { decodePosition, encodePosition } from "../reader/position";
import { flattenToc } from "../reader/toc";
import type { BookManifest } from "../reader/types";

const route = useRoute();

const bookId = computed(() => (route.params.bookId as string) || "");
const fmt = computed(() => ((route.params.fmt as string) || "epub").toLowerCase());

const manifest = ref<BookManifest | null>(null);
const loadError = ref<string | null>(null);
const statusMessage = ref("");
const spineIndex = ref(0);
const showToc = ref(false);
const iframeEl = ref<HTMLIFrameElement | null>(null);

const tocEntries = computed(() => (manifest.value ? flattenToc(manifest.value.toc) : []));

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
    const m = await pollManifest();
    manifest.value = m;
    statusMessage.value = "";

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
</script>

<template>
  <div class="reader">
    <header class="toolbar">
      <button @click="showToc = !showToc" :disabled="!manifest">Contents</button>
      <button @click="prev" :disabled="spineIndex <= 0">◀ Prev</button>
      <span class="title">{{ manifest?.metadata?.title ?? "" }}</span>
      <button @click="next" :disabled="!manifest || spineIndex >= manifest.spine.length - 1">Next ▶</button>
    </header>

    <p v-if="!bookId" class="empty">Open a book via <code>/read/&lt;book_id&gt;/&lt;fmt&gt;</code>.</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <p v-else-if="statusMessage" class="status">{{ statusMessage }}</p>

    <nav v-if="showToc" class="toc">
      <ul>
        <li v-for="(entry, i) in tocEntries" :key="i" :style="{ paddingLeft: `${entry.depth}em` }">
          <a href="#" @click.prevent="goToTocEntry(entry.dest, entry.frag)">{{ entry.title }}</a>
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
