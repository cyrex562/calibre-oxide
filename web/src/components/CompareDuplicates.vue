<script setup lang="ts">
// Compare 2 to 8 copies of a book side by side, edit each copy's title, authors,
// file name and folder in place, and decide which to keep.
//
// All state and rules are in ../library/compare.ts (tested there); this file is
// the template, the PDF page drawing (pdf.js, lazily, one canvas a column) and
// the wiring. A page change cancels the renders still running.

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";

import type { CompareBook, ComparePair } from "../library/api";
import { cancelLibraryJob } from "../library/api";
import { fileSummary, formatBytes, formatMtime } from "../library/duplicates";
import { isDirty, pairLines, PREVIEW_WIDTH } from "../library/compare";
import type { CompareController, DraftField } from "../library/compare";
import { loadPdfJs, PDF_ASSET_OPTIONS, pdfUrl, renderPage, scaleForWidth } from "../reader/pdf";

const props = defineProps<{ controller: CompareController }>();
const emit = defineEmits<{ close: [] }>();

const c = props.controller;
const state = c.state;

const books = computed<CompareBook[]>(() => state.result?.books ?? []);
const label = (i: number) => String.fromCharCode(65 + i);
const labelOf = (id: number) => label(books.value.findIndex((b) => b.book_id === id));
const hasPdf = computed(() => books.value.some((b) => (b.pdf?.pages ?? 0) > 0));
const searchText = ref("");
const mergeOthers = ref(false);
const busy = computed(() => state.loading || state.decision.phase === "working");

const pageBox = computed({
  get: () => c.pageNumber(),
  set: (n: number) => c.goTo(Number(n)),
});
const fromEnd = computed({
  get: () => state.cursor.anchor === "end",
  set: (on: boolean) => c.setAlignFromEnd(on),
});

function isPdf(b: CompareBook): boolean {
  return (b.pdf?.pages ?? 0) > 0 && (b.format_list ?? []).some((f) => f.toUpperCase() === "PDF");
}

function fileName(b: CompareBook): string {
  const p = b.files?.[0]?.path ?? "";
  return p.replace(/\\/g, "/").split("/").pop() ?? "";
}

function textLayer(b: CompareBook): string {
  if (!b.pdf) return "";
  if (b.pdf.has_text) return "text layer";
  return b.pdf.image_only || b.pdf.scanned ? "image only" : "no text";
}

function pairName(p: ComparePair): string {
  return `${labelOf(p.a)} and ${labelOf(p.b)}`;
}

function pairClass(p: ComparePair): string {
  const s = c.pairStatus(p);
  return s ? `on-${s}` : "";
}

function dirty(id: number, field: DraftField): boolean {
  const d = state.drafts[id];
  return !!d && isDirty(d, field);
}

async function decide() {
  await c.keepAndTrash({ mergeOthers: mergeOthers.value, confirm: (summary) => window.confirm(summary) });
}

function cancelWork() {
  if (state.decision.jobId !== null) void cancelLibraryJob(state.decision.jobId);
}

async function add(id: number) {
  await c.addBook(id);
  searchText.value = "";
}

// ---------------------------------------------------------------------------
// Drawing the pages
// ---------------------------------------------------------------------------

const canvases = new Map<number, HTMLCanvasElement>();
const docs = new Map<number, Promise<PDFDocumentProxy>>();
const tasks = new Map<number, { cancel: () => void }>();
const tokens = new Map<number, number>();
const rendering = ref<Record<number, boolean>>({});
const renderError = ref<Record<number, string>>({});
const visible = new Map<number, boolean>();
let observer: IntersectionObserver | null = null;

function setCanvas(id: number, el: unknown) {
  if (el instanceof HTMLCanvasElement) canvases.set(id, el);
  else canvases.delete(id);
}

function setPreview(id: number, el: unknown) {
  if (!(el instanceof HTMLElement)) return;
  el.dataset.bookId = String(id);
  observer?.observe(el);
}

async function documentFor(id: number): Promise<PDFDocumentProxy> {
  let doc = docs.get(id);
  if (!doc) {
    doc = loadPdfJs().then((pdfjs) => pdfjs.getDocument({ url: pdfUrl(String(id)), ...PDF_ASSET_OPTIONS }).promise);
    docs.set(id, doc);
    doc.catch(() => docs.delete(id));
  }
  return doc;
}

function isCancel(e: unknown): boolean {
  return e instanceof Error && e.name === "RenderingCancelledException";
}

/** Draws the page every column shows, stopping the one it replaces. */
async function draw(id: number) {
  const page = c.pageFor(id);
  const canvas = canvases.get(id);
  if (page === null || !canvas || visible.get(id) === false) return;
  const token = (tokens.get(id) ?? 0) + 1;
  tokens.set(id, token);
  tasks.get(id)?.cancel();
  rendering.value = { ...rendering.value, [id]: true };
  try {
    const doc = await documentFor(id);
    if (tokens.get(id) !== token) return;
    const first = await doc.getPage(page);
    const scale = scaleForWidth(first.getViewport({ scale: 1 }).width, PREVIEW_WIDTH);
    if (tokens.get(id) !== token) return;
    await renderPage(doc, page, { canvas, scale, onTask: (t) => tasks.set(id, t) });
    if (tokens.get(id) === token) renderError.value = { ...renderError.value, [id]: "" };
  } catch (e) {
    if (!isCancel(e) && tokens.get(id) === token) renderError.value = { ...renderError.value, [id]: e instanceof Error ? e.message : String(e) };
  } finally {
    if (tokens.get(id) === token) rendering.value = { ...rendering.value, [id]: false };
  }
}

function drawAll() {
  for (const b of books.value) if (isPdf(b)) void draw(b.book_id);
}

function forget(ids: number[]) {
  for (const id of ids) {
    tasks.get(id)?.cancel();
    tasks.delete(id);
    const doc = docs.get(id);
    docs.delete(id);
    void doc?.then((d) => d.destroy()).catch(() => undefined);
  }
}

watch(
  () => [state.cursor.anchor, state.cursor.offset, books.value.map((b) => `${b.book_id}:${b.pdf?.pages ?? 0}:${b.files?.[0]?.path ?? ""}`).join("|")],
  async () => {
    await nextTick();
    drawAll();
  },
);

watch(
  () => books.value.map((b) => b.book_id),
  (now, before) => forget((before ?? []).filter((id) => !now.includes(id))),
);

watch(
  () => state.open,
  (open) => {
    if (!open) forget([...docs.keys()]);
  },
);

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && !busy.value) emit("close");
}

onMounted(() => {
  window.addEventListener("keydown", onKey);
  if (typeof IntersectionObserver !== "undefined") {
    // A column scrolled out of view does not draw until it is in view.
    observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const id = Number((entry.target as HTMLElement).dataset.bookId);
        const was = visible.get(id);
        visible.set(id, entry.isIntersecting);
        if (entry.isIntersecting && was !== true) void draw(id);
      }
    });
  }
  void nextTick(drawAll);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  observer?.disconnect();
  forget([...docs.keys()]);
});
</script>

<template>
  <div class="cmp-overlay" role="dialog" aria-modal="true" aria-label="Compare copies" data-testid="compare-view">
    <header class="cmp-top">
      <h3>Compare {{ books.length || state.bookIds.length }} copies</h3>
      <span v-if="state.manualGroupId" class="cmp-badge" data-testid="your-group">Your group</span>
      <span class="cmp-spacer"></span>
      <button type="button" class="cmp-x" :disabled="busy" aria-label="Close" @click="emit('close')">✕</button>
    </header>

    <p v-if="state.error" class="cmp-error" role="alert" data-testid="compare-error">{{ state.error }}</p>
    <p v-if="state.note" class="cmp-hint" role="status">{{ state.note }}</p>
    <p v-if="state.loading && !state.result" role="status">Loading…</p>

    <template v-if="state.result">
      <section class="cmp-bar" aria-label="Page and group controls">
        <div v-if="hasPdf" class="cmp-pages" role="group" aria-label="Page">
          <button type="button" data-testid="page-first" @click="c.first()">First</button>
          <button type="button" data-testid="page-middle" @click="c.middle()">Middle</button>
          <button type="button" data-testid="page-last" @click="c.last()">Last</button>
          <button type="button" aria-label="Previous page" data-testid="page-prev" @click="c.step(-1)">‹</button>
          <label>
            Page
            <input v-model.number="pageBox" type="number" min="1" class="cmp-page-input" aria-label="Page number" data-testid="page-number" />
            <template v-if="fromEnd">from the end</template>
          </label>
          <button type="button" aria-label="Next page" data-testid="page-next" @click="c.step(1)">›</button>
          <label title="A scan can gain or lose a cover: count pages from the end of each file instead"><input v-model="fromEnd" type="checkbox" data-testid="align-end" /> Align from the end</label>
        </div>

        <div class="cmp-group-tools">
          <form class="cmp-add" @submit.prevent="c.searchBooks(searchText)">
            <input v-model="searchText" type="search" placeholder="Add a book… (search the library)" aria-label="Search for a book to add" data-testid="add-search" :disabled="state.bookIds.length >= 8" />
            <button type="submit" :disabled="!searchText.trim() || state.search.busy">Search</button>
          </form>
          <button v-if="!state.manualGroupId" type="button" :disabled="state.groupBusy" title="Keep these books together as a group of your own in the review list" data-testid="save-group" @click="c.saveAsGroup()">Save as my group</button>
          <button type="button" title="Never suggest these books as duplicates of each other again" data-testid="not-duplicates" @click="c.notDuplicates()">Not duplicates</button>
        </div>
      </section>

      <ul v-if="state.search.results.length || state.search.error" class="cmp-results" data-testid="add-results">
        <li v-if="state.search.error" class="cmp-error">{{ state.search.error }}</li>
        <li v-for="r in state.search.results" :key="r.id">
          <span>{{ r.title }} <span class="cmp-hint">{{ r.authors.join(", ") }}</span></span>
          <button type="button" @click="add(r.id)">Add</button>
        </li>
      </ul>

      <section v-if="state.result.pairs.length" class="cmp-evidence" aria-label="How the copies compare" data-testid="evidence">
        <div v-for="p in state.result.pairs" :key="`${p.a}-${p.b}`" class="cmp-pair" :class="pairClass(p)" data-testid="pair">
          <strong>{{ pairName(p) }}</strong>
          <span v-for="(line, i) in pairLines(p)" :key="i" class="cmp-pair-line">{{ line }}</span>
          <span v-if="c.pairStatus(p) === 'matched'" class="cmp-on" data-testid="pair-matched">These pages are a matched sample pair</span>
          <span v-else-if="c.pairStatus(p) === 'different'" class="cmp-off">These pages were compared and differ</span>
        </div>
        <p v-if="state.result.pdf.books > state.result.pdf.fingerprinted" class="cmp-hint">{{ state.result.pdf.books - state.result.pdf.fingerprinted }} PDF(s) not fingerprinted yet, so their pages cannot be compared.</p>
      </section>

      <div class="cmp-columns" data-testid="columns">
        <article v-for="(b, i) in books" :key="b.book_id" class="cmp-col" :class="{ keeper: state.keeperId === b.book_id }" data-testid="column">
          <header class="cmp-col-head">
            <strong class="cmp-letter">{{ label(i) }}</strong>
            <span v-if="state.keeperId === b.book_id" class="cmp-keep" data-testid="keeper-mark">{{ state.keeperChosen ? "Your choice to keep" : "Suggested to keep" }}</span>
            <span class="cmp-spacer"></span>
            <button v-if="state.keeperId !== b.book_id" type="button" data-testid="keep-this" @click="c.chooseKeeper(b.book_id)">Keep this</button>
            <button type="button" title="Take this book out of the comparison" :disabled="books.length <= 2" data-testid="remove-book" @click="c.removeBook(b.book_id)">Remove</button>
          </header>
          <p v-if="state.keeperId === b.book_id && state.result.keeper_reason && !state.keeperChosen" class="cmp-hint">{{ state.result.keeper_reason }}</p>

          <div :ref="(el) => setPreview(b.book_id, el)" class="cmp-preview" :style="{ width: `${PREVIEW_WIDTH}px` }">
            <template v-if="isPdf(b)">
              <canvas :ref="(el) => setCanvas(b.book_id, el)" :aria-label="`Page ${c.pageFor(b.book_id)} of ${b.title}`" data-testid="page-canvas"></canvas>
              <p class="cmp-page-label">page {{ c.pageFor(b.book_id) }} of {{ b.pdf?.pages }}<template v-if="rendering[b.book_id]"> · drawing…</template></p>
              <p v-if="renderError[b.book_id]" class="cmp-error">{{ renderError[b.book_id] }}</p>
            </template>
            <img v-else-if="b.cover_url" :src="b.cover_url" :alt="`Cover of ${b.title}`" data-testid="cover-image" />
            <p v-else class="cmp-hint">No preview ({{ (b.format_list ?? []).join(", ") || "no file" }}).</p>
          </div>

          <dl class="cmp-facts" data-testid="facts">
            <dt>File</dt>
            <dd :title="b.files?.[0]?.path">{{ fileName(b) || "—" }}</dd>
            <dt>Folder</dt>
            <dd>{{ b.path || "(library root)" }}</dd>
            <dt>Size</dt>
            <dd>{{ formatBytes(b.size ?? 0) }} <span class="cmp-hint">{{ fileSummary(b) }}</span></dd>
            <template v-if="b.pdf">
              <dt>Pages</dt>
              <dd>{{ b.pdf.pages }}</dd>
              <dt>Text</dt>
              <dd :class="{ warn: !b.pdf.has_text }">{{ textLayer(b) }}</dd>
            </template>
            <dt>Modified</dt>
            <dd>{{ formatMtime(b.mtime) || "—" }}</dd>
          </dl>

          <form v-if="state.drafts[b.book_id]" class="cmp-edit" @submit.prevent>
            <label>
              Title
              <span class="cmp-row">
                <input v-model="state.drafts[b.book_id].title" type="text" data-testid="edit-title" @keydown.enter.prevent="c.saveMetadata(b.book_id)" />
              </span>
            </label>
            <p v-if="state.drafts[b.book_id].errors.title" class="cmp-error" role="alert">{{ state.drafts[b.book_id].errors.title }}</p>
            <label>
              Authors <span class="cmp-hint">(separate with &amp;)</span>
              <input v-model="state.drafts[b.book_id].authors" type="text" data-testid="edit-authors" @keydown.enter.prevent="c.saveMetadata(b.book_id)" />
            </label>
            <p v-if="state.drafts[b.book_id].errors.authors" class="cmp-error" role="alert">{{ state.drafts[b.book_id].errors.authors }}</p>
            <div class="cmp-row">
              <button type="button" :disabled="!(dirty(b.book_id, 'title') || dirty(b.book_id, 'authors')) || state.drafts[b.book_id].saving.title || state.drafts[b.book_id].saving.authors" data-testid="save-metadata" @click="c.saveMetadata(b.book_id)">Save</button>
              <button type="button" title="Put this title and these authors in the other columns' boxes (they still need Save)" data-testid="use-title-for-all" @click="c.useTitleForAll(b.book_id)">Use this title for all</button>
            </div>

            <label>
              File name <span class="cmp-hint">(without .{{ (b.files?.[0]?.format ?? "").toLowerCase() || "ext" }})</span>
              <span class="cmp-row">
                <input v-model="state.drafts[b.book_id].stem" type="text" data-testid="edit-stem" @keydown.enter.prevent="c.saveStem(b.book_id)" />
                <button type="button" :disabled="!dirty(b.book_id, 'stem') || state.drafts[b.book_id].saving.stem" data-testid="save-stem" @click="c.saveStem(b.book_id)">Rename</button>
              </span>
            </label>
            <p v-if="state.drafts[b.book_id].errors.stem" class="cmp-error" role="alert">{{ state.drafts[b.book_id].errors.stem }}</p>

            <label>
              Folder <span class="cmp-hint">(inside the library)</span>
              <span class="cmp-row">
                <input v-model="state.drafts[b.book_id].folder" type="text" data-testid="edit-folder" @keydown.enter.prevent="c.moveBook(b.book_id)" />
                <button type="button" :disabled="!dirty(b.book_id, 'folder') || state.drafts[b.book_id].saving.folder" data-testid="move-book" @click="c.moveBook(b.book_id)">Move</button>
              </span>
            </label>
            <p v-if="state.drafts[b.book_id].errors.folder" class="cmp-error" role="alert">{{ state.drafts[b.book_id].errors.folder }}</p>
            <p v-if="state.drafts[b.book_id].moveNote" class="cmp-hint" role="status">
              {{ state.drafts[b.book_id].moveNote }}
              <button v-if="state.drafts[b.book_id].move" type="button" :disabled="state.drafts[b.book_id].saving.folder" data-testid="undo-move" @click="c.undoMove(b.book_id)">Undo move</button>
            </p>
          </form>
        </article>
      </div>
    </template>

    <footer v-if="state.result" class="cmp-decide" data-testid="decide">
      <template v-if="state.decision.phase === 'done' && state.decision.runId">
        <span role="status" data-testid="decision-result">{{ state.decision.message }}</span>
        <button type="button" data-testid="undo-decision" @click="c.undoDecision()">Undo</button>
        <button type="button" @click="emit('close')">Done</button>
      </template>
      <template v-else>
        <label title="Fold the other copies' tags, identifiers and other missing fields into the kept copy before they are trashed"><input v-model="mergeOthers" type="checkbox" data-testid="merge-others" /> Merge the others' metadata into the kept copy</label>
        <span class="cmp-spacer"></span>
        <span v-if="state.decision.phase === 'working'" role="status">{{ state.decision.progress || state.decision.message }}</span>
        <button v-if="state.decision.jobId !== null" type="button" @click="cancelWork">Cancel</button>
        <button type="button" class="primary" :disabled="busy || state.keeperId === null || books.length < 2" data-testid="keep-and-trash" @click="decide">Keep selected, trash the others</button>
      </template>
      <span v-if="state.decision.message && state.decision.phase === 'idle'" class="cmp-hint" role="status">{{ state.decision.message }}</span>
    </footer>
  </div>
</template>

<style scoped>
.cmp-overlay {
  position: fixed;
  inset: 0;
  z-index: 60;
  background: var(--bg);
  color: var(--fg);
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
  padding: 0.75rem 1rem;
  overflow: hidden;
}
.cmp-top {
  display: flex;
  align-items: center;
  gap: 0.6rem;
}
.cmp-top h3 {
  margin: 0;
}
.cmp-spacer {
  flex: 1;
}
.cmp-badge {
  border: 1px solid #1a7f37;
  color: #1a7f37;
  border-radius: 999px;
  padding: 0 0.5rem;
  font-size: 0.75em;
  font-weight: 600;
}
.cmp-hint {
  color: var(--fg-muted);
  font-size: var(--fs-body);
  margin: 0;
}
.cmp-error {
  color: var(--error, #cf222e);
  margin: 0;
}
.warn {
  color: #bc4c00;
}
.cmp-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 0.6rem 1.2rem;
  align-items: center;
}
.cmp-pages,
.cmp-group-tools,
.cmp-add,
.cmp-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.35rem;
  align-items: center;
}
.cmp-page-input {
  width: 4.5em;
}
.cmp-results {
  list-style: none;
  margin: 0;
  padding: 0.3rem 0.5rem;
  border: 1px solid var(--border);
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
  max-height: 9rem;
  overflow: auto;
}
.cmp-results li {
  display: flex;
  justify-content: space-between;
  gap: 0.6rem;
}
.cmp-evidence {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  max-height: 8.5rem;
  overflow: auto;
}
.cmp-pair {
  display: flex;
  flex-wrap: wrap;
  gap: 0.2rem 0.8rem;
  align-items: baseline;
  border-left: 3px solid transparent;
  padding-left: 0.5rem;
}
.cmp-pair.on-matched {
  border-left-color: #1a7f37;
  background: rgba(26, 127, 55, 0.08);
}
.cmp-pair.on-different {
  border-left-color: #bc4c00;
}
.cmp-pair-line {
  color: var(--fg-muted);
}
.cmp-on {
  color: #1a7f37;
  font-weight: 600;
}
.cmp-off {
  color: #bc4c00;
}
.cmp-columns {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 0.8rem;
  overflow: auto;
  align-items: flex-start;
}
.cmp-col {
  flex: 0 0 auto;
  width: 450px;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}
.cmp-col.keeper {
  border-color: #1a7f37;
  box-shadow: 0 0 0 1px #1a7f37 inset;
}
.cmp-col-head {
  display: flex;
  align-items: center;
  gap: 0.4rem;
}
.cmp-letter {
  font-size: 1.1em;
}
.cmp-keep {
  color: #1a7f37;
  font-weight: 600;
  font-size: 0.85em;
}
.cmp-preview {
  background: rgba(127, 127, 127, 0.1);
  min-height: 120px;
  display: flex;
  flex-direction: column;
  align-items: center;
}
.cmp-preview canvas,
.cmp-preview img {
  max-width: 100%;
  background: #fff;
}
.cmp-page-label {
  margin: 0.2rem 0;
  font-size: 0.8em;
  color: var(--fg-muted);
}
.cmp-facts {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 0.1rem 0.6rem;
  margin: 0;
  font-size: 0.9em;
}
.cmp-facts dt {
  color: var(--fg-muted);
}
.cmp-facts dd {
  margin: 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cmp-edit {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
}
.cmp-edit label {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  font-size: 0.9em;
}
.cmp-edit input[type="text"] {
  flex: 1;
  min-width: 0;
}
.cmp-decide {
  display: flex;
  flex-wrap: wrap;
  gap: 0.6rem;
  align-items: center;
  border-top: 1px solid var(--border);
  padding-top: 0.5rem;
}
.cmp-x {
  font-size: 1.1em;
}
</style>
