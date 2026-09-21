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

import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";

import { clampPage, clampScale, loadPdfJs, PDF_ASSET_OPTIONS, pdfUrl, renderPage } from "../reader/pdf";
import { getLastReadPositions, setLastReadPosition } from "../reader/api";
import { deviceId } from "../reader/position";

const props = defineProps<{ bookId: string }>();

const canvas = ref<HTMLCanvasElement | null>(null);
// `shallowRef`, not `ref`: a deep reactive proxy over a PDF document
// both reshapes its type (Vue's unwrapping strips the class's private
// fields, so it no longer satisfies `PDFDocumentProxy`) and would
// walk a very large object graph for no benefit. Nothing here needs
// reactivity *inside* the document -- only on which document it is.
const doc = shallowRef<PDFDocumentProxy | null>(null);
const pageCount = ref(0);
const page = ref(1);
const scale = ref(1);
const loading = ref(true);
const error = ref<string | null>(null);

// PDF.js renders asynchronously; a fast click-through would otherwise
// interleave two renders onto one canvas and leave whichever finished
// last on screen, which is not necessarily the page asked for.
let renderToken = 0;

async function draw() {
  const d = doc.value;
  const el = canvas.value;
  if (!d || !el) return;

  const token = ++renderToken;
  try {
    await renderPage(d, page.value, { canvas: el, scale: scale.value });
    if (token !== renderToken) return; // superseded mid-render
    error.value = null;
  } catch (e) {
    if (token !== renderToken) return;
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const pdfjs = await loadPdfJs();
    const task = pdfjs.getDocument({ url: pdfUrl(props.bookId), ...PDF_ASSET_OPTIONS });
    const loaded = await task.promise;
    doc.value = loaded;
    pageCount.value = loaded.numPages;

    // Restore where we were, if anything was stored.
    try {
      // Positions come back one per device; prefer this device's own.
      const positions = await getLastReadPositions(props.bookId, "pdf");
      const mine = positions.find((p) => p.device === deviceId()) ?? positions[0];
      const restored = mine?.cfi ? Number.parseInt(mine.cfi, 10) : Number.NaN;
      page.value = clampPage(restored, loaded.numPages);
    } catch {
      page.value = 1;
    }

    await draw();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

function go(delta: number) {
  const next = clampPage(page.value + delta, pageCount.value);
  if (next !== page.value) page.value = next;
}

function setPage(value: number) {
  page.value = clampPage(value, pageCount.value);
}

function zoom(delta: number) {
  scale.value = clampScale(Math.round((scale.value + delta) * 100) / 100);
}

function onKeydown(event: KeyboardEvent) {
  if (event.target instanceof HTMLInputElement) return;
  if (event.key === "ArrowRight" || event.key === "PageDown") go(1);
  else if (event.key === "ArrowLeft" || event.key === "PageUp") go(-1);
  else return;
  event.preventDefault();
}

// The page number doubles as the stored position: PDFs have no CFI,
// and a page is the only location a reader and a file agree on.
watch(page, (value) => {
  void draw();
  void setLastReadPosition(props.bookId, "pdf", deviceId(), String(value), pageCount.value > 1 ? (value - 1) / (pageCount.value - 1) : 0).catch(() => {});
});
watch(scale, () => void draw());
watch(() => props.bookId, () => void load());

const progressLabel = computed(() => (pageCount.value ? `${page.value} / ${pageCount.value}` : ""));

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  void load();
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  // Frees the worker's copy of the document; without this, opening
  // several PDFs in a session leaks each one.
  void doc.value?.destroy();
});
</script>

<template>
  <div class="pdf-reader">
    <div class="pdf-toolbar">
      <button type="button" :disabled="page <= 1" @click="go(-1)">◀ Prev</button>
      <label class="pdf-page">
        Page
        <input type="number" :value="page" min="1" :max="pageCount || 1" @change="setPage(Number(($event.target as HTMLInputElement).value))" />
        <span>/ {{ pageCount || "?" }}</span>
      </label>
      <button type="button" :disabled="pageCount === 0 || page >= pageCount" @click="go(1)">Next ▶</button>
      <span class="pdf-zoom">
        <button type="button" title="Zoom out" @click="zoom(-0.25)">−</button>
        <span class="pdf-scale">{{ Math.round(scale * 100) }}%</span>
        <button type="button" title="Zoom in" @click="zoom(0.25)">+</button>
      </span>
      <span class="pdf-progress">{{ progressLabel }}</span>
    </div>

    <p v-if="loading" class="status">Loading PDF…</p>
    <p v-if="error" class="error">{{ error }}</p>

    <div class="pdf-page-area">
      <canvas ref="canvas" />
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
.pdf-toolbar {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid #ddd;
  flex-wrap: wrap;
}
.pdf-page {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  font-size: 0.85rem;
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
.pdf-scale {
  font-variant-numeric: tabular-nums;
  font-size: 0.85rem;
  min-width: 3.5rem;
  text-align: center;
}
.pdf-progress {
  margin-left: auto;
  font-size: 0.85rem;
  opacity: 0.7;
  font-variant-numeric: tabular-nums;
}
.pdf-page-area {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  justify-content: center;
  align-items: flex-start;
  padding: 1rem;
  background: #525659;
}
canvas {
  box-shadow: 0 2px 10px rgb(0 0 0 / 35%);
  background: #fff;
}
@media (prefers-color-scheme: dark) {
  .pdf-toolbar {
    border-bottom-color: #3a3d44;
  }
}
</style>
