<script setup lang="ts">
// Compare 2 to 8 copies of a book side by side, edit each copy's title, authors,
// file name and folder in place, and decide which to keep.
//
// All state and rules are in ../library/compare.ts (tested there); this file is
// the template, the PDF page drawing (pdf.js, lazily, one canvas a column) and
// the wiring. A page change cancels the renders still running.

import Dialog from "./Dialog.vue";
import { confirmDialog } from "./dialogs/confirm";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";

import type { CompareBook, ComparePair } from "../library/api";
import { cancelLibraryJob } from "../library/api";
import { offerRunUndo } from "../library/dedupe";
import { factRows, isDirty, pairLines, PREVIEW_WIDTH } from "../library/compare";
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

const facts = computed(() => factRows(books.value));
const notAnalyzed = computed(() => (state.result ? state.result.pdf.books - state.result.pdf.fingerprinted : 0));

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
  await c.keepAndTrash({ mergeOthers: mergeOthers.value, confirm: (summary) => confirmDialog(summary, { title: "Keep and trash", confirmLabel: "Continue", danger: true }) });
}

/**
 * Closing after a keep-and-trash: the footer's Undo goes away with the dialog,
 * so the same Undo is offered as a toast (the one every de-dup action shows).
 */
function closeView() {
  const d = state.decision;
  if (d.phase === "done" && d.runId && d.trashedIds.length > 0) offerRunUndo(d.message, d.runId, () => c.refreshAfterUndo());
  emit("close");
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

onMounted(() => {
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
  observer?.disconnect();
  forget([...docs.keys()]);
});
</script>

<template>
  <Dialog size="full" label="Compare copies" :busy="busy" show-close class="cmp-dialog" data-testid="compare-view" @close="closeView">
    <div class="cmp-overlay">
    <header class="cmp-top">
      <h3>Compare {{ books.length || state.bookIds.length }} copies</h3>
      <span v-if="state.manualGroupId" class="cmp-badge" data-testid="your-group" title="You marked these books as duplicates of each other">Marked as duplicates</span>
    </header>

    <p v-if="state.error" class="cmp-error" role="alert" data-testid="compare-error">{{ state.error }}</p>
    <p v-if="state.note" class="cmp-hint" role="status">{{ state.note }}</p>
    <p v-if="state.loading && !state.result" role="status">Loading…</p>

    <template v-if="state.result">
      <!--
        The facts that tell the copies apart, one row per fact and one column per
        copy, above the page images so they are on screen when it opens. A row the
        copies disagree on is highlighted.
      -->
      <table class="cmp-table" data-testid="facts" aria-label="The copies side by side">
        <thead>
          <tr>
            <th scope="col"><span class="cmp-sr">Fact</span></th>
            <th v-for="(b, i) in books" :key="b.book_id" scope="col" :class="{ keeper: state.keeperId === b.book_id }">
              {{ label(i) }}<span v-if="state.keeperId === b.book_id" class="cmp-keep"> · keep</span>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="row in facts" :key="row.key" :class="{ differs: row.differs }" :data-fact="row.key">
            <th scope="row">{{ row.label }}</th>
            <td v-for="(v, i) in row.values" :key="i" :title="v">{{ v }}</td>
          </tr>
        </tbody>
      </table>
      <p v-if="state.result.keeper_reason && !state.keeperChosen && state.keeperId !== null" class="cmp-hint" data-testid="keeper-reason">Suggested to keep {{ labelOf(state.keeperId) }}: {{ state.result.keeper_reason }}.</p>

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
            <input v-model="searchText" type="search" placeholder="Add a book…" aria-label="Search for a book to add" data-testid="add-search" :disabled="state.bookIds.length >= 8" />
            <button type="submit" :disabled="!searchText.trim() || state.search.busy">Search</button>
          </form>
          <button v-if="!state.manualGroupId" type="button" :disabled="state.groupBusy" title="Keep these books together as one group in the Find duplicates list, even if the scan would not match them" data-testid="save-group" @click="c.saveAsGroup()">Mark these as duplicates</button>
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
        <p v-if="notAnalyzed > 0" class="cmp-hint" data-testid="not-analyzed">{{ notAnalyzed }} PDF{{ notAnalyzed === 1 ? "" : "s" }} not analyzed yet, so {{ notAnalyzed === 1 ? "its" : "their" }} pages cannot be compared. Use “Analyze library” in Find duplicates.</p>
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

          <div :ref="(el) => setPreview(b.book_id, el)" class="cmp-preview" :style="{ maxWidth: `${PREVIEW_WIDTH}px` }">
            <template v-if="isPdf(b)">
              <canvas :ref="(el) => setCanvas(b.book_id, el)" :aria-label="`Page ${c.pageFor(b.book_id)} of ${b.title}`" data-testid="page-canvas"></canvas>
              <p class="cmp-page-label">page {{ c.pageFor(b.book_id) }} of {{ b.pdf?.pages }}<template v-if="rendering[b.book_id]"> · drawing…</template></p>
              <p v-if="renderError[b.book_id]" class="cmp-error">{{ renderError[b.book_id] }}</p>
            </template>
            <img v-else-if="b.cover_url" :src="b.cover_url" :alt="`Cover of ${b.title}`" data-testid="cover-image" />
            <p v-else class="cmp-hint">No preview ({{ (b.format_list ?? []).join(", ") || "no file" }}).</p>
          </div>

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

    </div>
    <template v-if="state.result" #footer>
      <template v-if="state.decision.phase === 'done' && state.decision.runId">
        <span role="status" data-testid="decision-result">{{ state.decision.message }}</span>
        <button type="button" data-testid="undo-decision" @click="c.undoDecision()">Undo</button>
        <button type="button" class="primary" @click="closeView">Done</button>
      </template>
      <template v-else>
        <label title="Fold the other copies' tags, identifiers and other missing fields into the kept copy before they are trashed"><input v-model="mergeOthers" type="checkbox" data-testid="merge-others" /> Merge the others' metadata into the kept copy</label>
        <span class="cmp-spacer"></span>
        <span v-if="state.decision.phase === 'working'" role="status">{{ state.decision.progress || state.decision.message }}</span>
        <button v-if="state.decision.jobId !== null" type="button" @click="cancelWork">Cancel</button>
        <button type="button" class="primary" :disabled="busy || state.keeperId === null || books.length < 2" data-testid="keep-and-trash" @click="decide">Keep selected, trash the others</button>
      </template>
      <span v-if="state.decision.message && state.decision.phase === 'idle'" class="cmp-hint" role="status">{{ state.decision.message }}</span>
    </template>
  </Dialog>
</template>

<style scoped>
.cmp-overlay {
  flex: 1;
  min-height: 0;
  color: var(--fg);
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
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
  border: 1px solid var(--success);
  color: var(--success);
  border-radius: var(--radius-pill);
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
  color: var(--danger);
  margin: 0;
}
.warn {
  color: var(--warning);
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
  border-radius: var(--radius-md);
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
  border-left-color: var(--success);
  background: rgba(26, 127, 55, 0.08);
}
.cmp-pair.on-different {
  border-left-color: var(--warning);
}
.cmp-pair-line {
  color: var(--fg-muted);
}
.cmp-on {
  color: var(--success);
  font-weight: 600;
}
.cmp-off {
  color: var(--warning);
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
  /* The copies share the whole width instead of sitting in fixed 450px columns. */
  flex: 1 1 0;
  min-width: 300px;
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  padding: 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}
.cmp-col.keeper {
  border-color: var(--success);
  box-shadow: 0 0 0 1px var(--success) inset;
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
  color: var(--success);
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
.cmp-table {
  width: 100%;
  table-layout: fixed;
  border-collapse: collapse;
  font-size: 0.9em;
  flex: 0 0 auto;
}
.cmp-table th,
.cmp-table td {
  text-align: left;
  padding: 0.15rem 0.5rem;
  border-bottom: 1px solid var(--border);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cmp-table thead th:first-child,
.cmp-table tbody th {
  width: 6.5rem;
  color: var(--fg-muted);
  font-weight: normal;
}
.cmp-table thead th.keeper {
  color: var(--success);
}
.cmp-table tr.differs td {
  font-weight: 600;
  background: rgba(154, 103, 0, 0.1);
}
.cmp-sr {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
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
</style>
