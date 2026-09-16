<script setup lang="ts">
import { computed, ref, watch } from "vue";
import CategoryBrowser from "./CategoryBrowser.vue";
import BookDetailsPanel from "./BookDetailsPanel.vue";
import { addBook, deleteSavedSearch, deleteVirtualLibrary, fetchBooks, fetchFieldMetadata, fetchSavedSearches, fetchVirtualLibraries, ftsSearch, ftsSnippets, renameSavedSearch, search, setFields, setFtsEnabled, setSavedSearch, setVirtualLibrary } from "../library/api";
import { parseSnippetSegments } from "../library/snippets";
import { isTauri, tauriInvoke } from "../tauri";
import type { BookFieldChanges, BookSummary, FtsSnippet } from "../library/types";

const PAGE_SIZE = 24;

const queryText = ref("");
const activeQuery = ref(""); // committed query -- what's actually sent, vs. the input box's live text
const sort = ref("timestamp");
const sortOrder = ref<"asc" | "desc">("desc");
const vl = ref("");
const offset = ref(0);

const sortableFields = ref<[string, string][]>([]);
const virtualLibraries = ref<Record<string, string>>({});
const savedSearches = ref<Record<string, string>>({});

const books = ref<BookSummary[]>([]);
const totalNum = ref(0);
const loading = ref(false);
const error = ref<string | null>(null);
const selectedBookId = ref<number | null>(null);
const cacheBust = ref(0);

const addInput = ref<HTMLInputElement | null>(null);
const adding = ref(false);
const addError = ref<string | null>(null);

const pageCount = computed(() => Math.max(1, Math.ceil(totalNum.value / PAGE_SIZE)));
const currentPage = computed(() => Math.floor(offset.value / PAGE_SIZE) + 1);

async function loadMetadata() {
  try {
    const [fm, vls, searches] = await Promise.all([fetchFieldMetadata(), fetchVirtualLibraries(), fetchSavedSearches()]);
    sortableFields.value = fm.sortable_fields;
    virtualLibraries.value = vls;
    savedSearches.value = searches;
  } catch (e) {
    // Non-fatal -- the grid itself still works with default sort/no vl.
    console.error("failed to load field metadata / virtual libraries / saved searches", e);
  }
}
void loadMetadata();

// Virtual library / saved search management -- real, new routes (see
// crates/calibre_srv/src/lists.rs's own doc for why no upstream route
// exists to port here).
const manageOpen = ref(false);
const manageError = ref<string | null>(null);
const newVlName = ref("");
const newVlQuery = ref("");
const newSearchName = ref("");
const newSearchQuery = ref("");

function openManage() {
  newVlQuery.value = activeQuery.value;
  newSearchQuery.value = activeQuery.value;
  manageError.value = null;
  manageOpen.value = true;
}

async function createVirtualLibrary() {
  if (!newVlName.value.trim() || !newVlQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setVirtualLibrary(newVlName.value.trim(), newVlQuery.value.trim());
    newVlName.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeVirtualLibrary(name: string) {
  if (!confirm(`Delete the virtual library "${name}"?`)) return;
  manageError.value = null;
  try {
    await deleteVirtualLibrary(name);
    if (vl.value === name) vl.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function createSavedSearch() {
  if (!newSearchName.value.trim() || !newSearchQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setSavedSearch(newSearchName.value.trim(), newSearchQuery.value.trim());
    newSearchName.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeSavedSearch(name: string) {
  if (!confirm(`Delete the saved search "${name}"?`)) return;
  manageError.value = null;
  try {
    await deleteSavedSearch(name);
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function renameSavedSearchPrompt(name: string) {
  const newName = prompt("Rename saved search to:", name);
  if (!newName || newName === name) return;
  manageError.value = null;
  try {
    await renameSavedSearch(name, newName);
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

function applySavedSearch(query: string) {
  queryText.value = query;
  activeQuery.value = query;
  offset.value = 0;
  manageOpen.value = false;
}

async function runSearch() {
  loading.value = true;
  error.value = null;
  try {
    const result = await search({
      query: activeQuery.value,
      num: PAGE_SIZE,
      offset: offset.value,
      sort: sort.value,
      sortOrder: sortOrder.value,
      vl: vl.value,
    });
    totalNum.value = result.total_num;
    books.value = await fetchBooks(result.book_ids);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
    books.value = [];
    totalNum.value = 0;
  } finally {
    loading.value = false;
  }
}

watch([activeQuery, sort, sortOrder, vl, offset], () => {
  if (!ftsMode.value) void runSearch();
}, { immediate: true });

// Full-text search -- a separate result shape/backend (crates/calibre_srv/src/fts.rs)
// from the metadata search above: no pagination/sort/vl, real
// snippet/highlight text per match, and a real "not enabled yet" state
// to surface instead of an empty result list.
interface FtsDisplayResult {
  bookId: number;
  title: string;
  authors: string;
  formats: string[];
  snippets: FtsSnippet[];
}

const ftsMode = ref(false);
const ftsLoading = ref(false);
const ftsError = ref<string | null>(null);
const ftsNotEnabled = ref(false);
const ftsEnabling = ref(false);
const ftsIndexing = ref<{ left: number; total: number } | null>(null);
const ftsResults = ref<FtsDisplayResult[]>([]);

async function runFtsSearch() {
  const query = activeQuery.value;
  ftsError.value = null;
  ftsNotEnabled.value = false;
  ftsIndexing.value = null;
  if (!query) {
    ftsResults.value = [];
    return;
  }
  ftsLoading.value = true;
  try {
    const outcome = await ftsSearch(query);
    if (!outcome.enabled) {
      ftsNotEnabled.value = true;
      ftsResults.value = [];
      return;
    }
    const { metadata, indexing_status, results } = outcome.result;
    ftsIndexing.value = indexing_status;

    const formatsByBook = new Map<number, string[]>();
    for (const r of results) {
      const list = formatsByBook.get(r.book_id) ?? [];
      if (!list.includes(r.format)) list.push(r.format);
      formatsByBook.set(r.book_id, list);
    }
    const bookIds = [...formatsByBook.keys()];
    const snippetsByBook = await ftsSnippets(bookIds, query);

    ftsResults.value = bookIds.map((bookId) => ({
      bookId,
      title: metadata[String(bookId)]?.title ?? `Book ${bookId}`,
      authors: metadata[String(bookId)]?.authors ?? "",
      formats: formatsByBook.get(bookId) ?? [],
      snippets: snippetsByBook[String(bookId)] ?? [],
    }));
  } catch (e) {
    ftsError.value = e instanceof Error ? e.message : String(e);
    ftsResults.value = [];
  } finally {
    ftsLoading.value = false;
  }
}

watch([activeQuery, ftsMode], () => {
  if (ftsMode.value) void runFtsSearch();
});

async function enableFts() {
  ftsEnabling.value = true;
  ftsError.value = null;
  try {
    await setFtsEnabled(true);
    await runFtsSearch();
  } catch (e) {
    ftsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    ftsEnabling.value = false;
  }
}

function snippetSegments(text: string) {
  return parseSnippetSegments(text);
}

function submitSearch() {
  activeQuery.value = queryText.value;
  offset.value = 0;
}

function onCategorySelect(categoryQuery: string) {
  queryText.value = categoryQuery;
  activeQuery.value = categoryQuery;
  offset.value = 0;
}

function nextPage() {
  if (offset.value + PAGE_SIZE < totalNum.value) offset.value += PAGE_SIZE;
}
function prevPage() {
  if (offset.value > 0) offset.value = Math.max(0, offset.value - PAGE_SIZE);
}

function onDetailsUpdated() {
  cacheBust.value++;
  void (ftsMode.value ? runFtsSearch() : runSearch());
}

function onDetailsDeleted() {
  selectedBookId.value = null;
  void (ftsMode.value ? runFtsSearch() : runSearch());
}

// Bulk (multi-book) metadata editing -- client-side fan-out over the
// existing per-book /cdb/set-fields, matching #716's own scope: no
// bulk server endpoint exists (or is needed yet), just N real
// requests with real per-book error aggregation so one bad book
// doesn't silently swallow the rest.
const selectMode = ref(false);
const selectedIds = ref<Set<number>>(new Set());
const bulkOpen = ref(false);
const bulkBusy = ref(false);
const bulkErrors = ref<string[]>([]);
const bulkAddTags = ref("");
const bulkRemoveTags = ref("");
const bulkSeries = ref("");
const bulkApplyRating = ref(false);
const bulkRating = ref(0);

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) {
    selectedIds.value = new Set();
    bulkOpen.value = false;
  }
}

function toggleSelected(bookId: number) {
  const next = new Set(selectedIds.value);
  if (next.has(bookId)) next.delete(bookId);
  else next.add(bookId);
  selectedIds.value = next;
}

function onCardClick(bookId: number) {
  if (selectMode.value) toggleSelected(bookId);
  else selectedBookId.value = bookId;
}

async function runBulkEdit() {
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;

  const addTags = bulkAddTags.value.split(",").map((t) => t.trim()).filter(Boolean);
  const removeTags = bulkRemoveTags.value.split(",").map((t) => t.trim()).filter(Boolean);
  const setSeries = bulkSeries.value.trim();

  bulkBusy.value = true;
  bulkErrors.value = [];
  for (const id of ids) {
    try {
      const changes: BookFieldChanges = {};
      if (addTags.length || removeTags.length) {
        const current = books.value.find((b) => b.id === id);
        const tags = new Set(current?.tags ?? []);
        for (const t of addTags) tags.add(t);
        for (const t of removeTags) tags.delete(t);
        changes.tags = [...tags];
      }
      if (setSeries) changes.series = setSeries;
      if (bulkApplyRating.value) changes.rating = bulkRating.value;
      if (Object.keys(changes).length === 0) continue;
      await setFields(id, changes);
    } catch (e) {
      bulkErrors.value.push(`Book ${id}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  bulkBusy.value = false;

  if (bulkErrors.value.length === 0) {
    bulkOpen.value = false;
    selectMode.value = false;
    selectedIds.value = new Set();
    bulkAddTags.value = "";
    bulkRemoveTags.value = "";
    bulkSeries.value = "";
    bulkApplyRating.value = false;
  }
  cacheBust.value++;
  await runSearch();
}

const addSummary = ref<string | null>(null);

async function addOneBookFile(file: File, addDuplicates: boolean): Promise<"added" | "skipped"> {
  const result = await addBook(file, addDuplicates);
  if (result.duplicates && result.duplicates.length > 0 && result.book_id === undefined) {
    const names = result.duplicates.map((d) => `${d.title} (${d.authors.join(" & ")})`).join(", ");
    if (confirm(`"${file.name}": a book with the same title/author already exists: ${names}. Add anyway?`)) {
      return addOneBookFile(file, true);
    }
    return "skipped";
  }
  return "added";
}

async function onAddFileSelected(e: Event) {
  const input = e.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  if (files.length === 0) return;
  adding.value = true;
  addError.value = null;
  addSummary.value = null;
  const errors: string[] = [];
  let added = 0;
  let skipped = 0;
  for (const file of files) {
    try {
      const outcome = await addOneBookFile(file, false);
      if (outcome === "added") added++;
      else skipped++;
    } catch (err) {
      errors.push(`${file.name}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  adding.value = false;
  if (addInput.value) addInput.value.value = "";
  if (errors.length > 0) addError.value = errors.join("; ");
  if (files.length > 1) {
    addSummary.value = `Added ${added} of ${files.length} book(s)${skipped ? `, ${skipped} skipped` : ""}${errors.length ? `, ${errors.length} failed` : ""}.`;
  }
  cacheBust.value++;
  await runSearch();
}

interface AddFolderResult {
  added: number;
  duplicates: string[];
  errors: string[];
}

const addingFolder = ref(false);

async function addFolder() {
  addingFolder.value = true;
  addError.value = null;
  addSummary.value = null;
  try {
    const result = await tauriInvoke<AddFolderResult | null>("choose_folder_and_add_books");
    if (!result) return; // dialog cancelled
    addSummary.value = `Added ${result.added} book(s) from the folder${result.duplicates.length ? `, skipped ${result.duplicates.length} duplicate(s)` : ""}${result.errors.length ? `, ${result.errors.length} failed` : ""}.`;
    if (result.errors.length > 0) addError.value = result.errors.join("; ");
    cacheBust.value++;
    await runSearch();
  } catch (e) {
    addError.value = e instanceof Error ? e.message : String(e);
  } finally {
    addingFolder.value = false;
  }
}
</script>

<template>
  <div class="library">
    <header class="toolbar">
      <form class="search" @submit.prevent="submitSearch">
        <input v-model="queryText" type="search" :placeholder="ftsMode ? 'Search book contents…' : 'Search…'" />
        <button type="submit">Search</button>
      </form>

      <button type="button" :class="{ active: ftsMode }" @click="ftsMode = !ftsMode">
        {{ ftsMode ? "Full-text search" : "Metadata search" }}
      </button>

      <template v-if="!ftsMode">
        <select v-model="sort">
          <option v-for="[key, label] in sortableFields" :key="key" :value="key">{{ label }}</option>
        </select>
        <select v-model="sortOrder">
          <option value="asc">Ascending</option>
          <option value="desc">Descending</option>
        </select>
        <select v-model="vl">
          <option value="">All books</option>
          <option v-for="name in Object.keys(virtualLibraries)" :key="name" :value="name">{{ name }}</option>
        </select>
        <select v-if="Object.keys(savedSearches).length" @change="applySavedSearch(($event.target as HTMLSelectElement).value)">
          <option value="" disabled selected>Saved searches…</option>
          <option v-for="[name, q] in Object.entries(savedSearches)" :key="name" :value="q">{{ name }}</option>
        </select>
        <button type="button" @click="openManage">Manage lists…</button>
      </template>

      <button type="button" :disabled="adding" @click="addInput?.click()">{{ adding ? "Adding…" : "Add Books…" }}</button>
      <input ref="addInput" type="file" multiple class="hidden-file-input" @change="onAddFileSelected" />
      <button v-if="isTauri()" type="button" :disabled="addingFolder" @click="addFolder">{{ addingFolder ? "Adding…" : "Add Folder…" }}</button>

      <template v-if="!ftsMode">
        <button type="button" :class="{ active: selectMode }" @click="toggleSelectMode">
          {{ selectMode ? "Cancel selection" : "Select…" }}
        </button>
        <button v-if="selectedIds.size > 0" type="button" @click="bulkOpen = true">Bulk edit ({{ selectedIds.size }})</button>
      </template>
    </header>

    <div v-if="bulkOpen" class="bulk-panel">
      <div class="bulk-row">
        <label>Add tags <input v-model="bulkAddTags" placeholder="scifi, classic" :disabled="bulkBusy" /></label>
        <label>Remove tags <input v-model="bulkRemoveTags" placeholder="unwanted-tag" :disabled="bulkBusy" /></label>
        <label>Set series <input v-model="bulkSeries" :disabled="bulkBusy" /></label>
        <label class="bulk-rating">
          <input type="checkbox" v-model="bulkApplyRating" :disabled="bulkBusy" />
          Set rating
          <input type="number" v-model.number="bulkRating" min="0" max="5" step="1" :disabled="bulkBusy || !bulkApplyRating" />
        </label>
      </div>
      <div class="bulk-actions">
        <button type="button" :disabled="bulkBusy" @click="runBulkEdit">
          {{ bulkBusy ? "Applying…" : `Apply to ${selectedIds.size} book(s)` }}
        </button>
        <button type="button" :disabled="bulkBusy" @click="bulkOpen = false">Close</button>
      </div>
      <ul v-if="bulkErrors.length" class="bulk-errors">
        <li v-for="(err, i) in bulkErrors" :key="i" class="error">{{ err }}</li>
      </ul>
    </div>

    <p v-if="addError" class="error add-error">{{ addError }}</p>
    <p v-if="addSummary" class="status add-summary">{{ addSummary }}</p>

    <div class="body">
      <CategoryBrowser class="sidebar" @select="onCategorySelect" />

      <main v-if="ftsMode" class="grid-area">
        <p v-if="ftsError" class="error">{{ ftsError }}</p>
        <div v-else-if="ftsNotEnabled" class="fts-enable">
          <p>Full-text search is not enabled on this library yet.</p>
          <button type="button" :disabled="ftsEnabling" @click="enableFts">
            {{ ftsEnabling ? "Enabling…" : "Enable full-text search" }}
          </button>
        </div>
        <template v-else>
          <p v-if="ftsIndexing && ftsIndexing.left > 0" class="status fts-indexing">
            Indexing… {{ ftsIndexing.left }} of {{ ftsIndexing.total }} remaining. Results may be incomplete until indexing finishes.
          </p>
          <p v-if="ftsLoading" class="status">Searching…</p>
          <p v-else-if="activeQuery && ftsResults.length === 0" class="status">No matches found.</p>
          <p v-else-if="!activeQuery" class="status">Enter a search to look through book contents.</p>

          <ul class="fts-results">
            <li v-for="r in ftsResults" :key="r.bookId" class="fts-result" @click="selectedBookId = r.bookId">
              <div class="fts-result-header">
                <span class="fts-result-title">{{ r.title }}</span>
                <span class="fts-result-authors">{{ r.authors }}</span>
                <span class="fts-result-formats">{{ r.formats.join(", ") }}</span>
              </div>
              <p v-for="(snippet, i) in r.snippets" :key="i" class="fts-snippet">
                <template v-for="(seg, j) in snippetSegments(snippet.text)" :key="j">
                  <mark v-if="seg.highlighted">{{ seg.text }}</mark>
                  <template v-else>{{ seg.text }}</template>
                </template>
              </p>
            </li>
          </ul>
        </template>
      </main>

      <main v-else class="grid-area">
        <p v-if="error" class="error">{{ error }}</p>
        <p v-else-if="loading" class="status">Loading…</p>
        <p v-else-if="books.length === 0" class="status">No books found.</p>

        <div class="grid">
          <button v-for="book in books" :key="book.id" class="card" :class="{ selected: selectMode && selectedIds.has(book.id) }" @click="onCardClick(book.id)">
            <input v-if="selectMode" type="checkbox" class="card-checkbox" :checked="selectedIds.has(book.id)" @click.stop="toggleSelected(book.id)" />
            <img :src="`${book.thumbnail}?v=${cacheBust}`" :alt="book.title" loading="lazy" />
            <div class="card-title">{{ book.title }}</div>
            <div class="card-authors">{{ (book.authors ?? []).join(" & ") }}</div>
          </button>
        </div>

        <footer class="pagination">
          <button :disabled="offset === 0" @click="prevPage">◀ Prev</button>
          <span>Page {{ currentPage }} / {{ pageCount }} ({{ totalNum }} books)</span>
          <button :disabled="offset + PAGE_SIZE >= totalNum" @click="nextPage">Next ▶</button>
        </footer>
      </main>
    </div>

    <BookDetailsPanel v-if="selectedBookId !== null" :book-id="selectedBookId" @close="selectedBookId = null" @updated="onDetailsUpdated" @deleted="onDetailsDeleted" />

    <div v-if="manageOpen" class="manage-backdrop" @click.self="manageOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="manageOpen = false">✕</button>
        <p v-if="manageError" class="error">{{ manageError }}</p>

        <section>
          <h3>Virtual libraries</h3>
          <ul class="manage-list">
            <li v-for="[name, q] in Object.entries(virtualLibraries)" :key="name">
              <span class="manage-name">{{ name }}</span>
              <code class="manage-query">{{ q }}</code>
              <button type="button" class="manage-remove" @click="removeVirtualLibrary(name)">Delete</button>
            </li>
          </ul>
          <form class="manage-form" @submit.prevent="createVirtualLibrary">
            <input v-model="newVlName" placeholder="Name" required />
            <input v-model="newVlQuery" placeholder="Search query" required />
            <button type="submit">Add</button>
          </form>
        </section>

        <section>
          <h3>Saved searches</h3>
          <ul class="manage-list">
            <li v-for="[name, q] in Object.entries(savedSearches)" :key="name">
              <span class="manage-name">{{ name }}</span>
              <code class="manage-query">{{ q }}</code>
              <button type="button" @click="renameSavedSearchPrompt(name)">Rename</button>
              <button type="button" class="manage-remove" @click="removeSavedSearch(name)">Delete</button>
            </li>
          </ul>
          <form class="manage-form" @submit.prevent="createSavedSearch">
            <input v-model="newSearchName" placeholder="Name" required />
            <input v-model="newSearchQuery" placeholder="Search query" required />
            <button type="submit">Add</button>
          </form>
        </section>
      </div>
    </div>
  </div>
</template>

<style scoped>
.library {
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
  flex-wrap: wrap;
}
.search {
  display: flex;
  flex: 1;
  min-width: 200px;
  gap: 0.25em;
}
.search input {
  flex: 1;
}
.body {
  flex: 1;
  display: flex;
  overflow: hidden;
}
.sidebar {
  width: 220px;
  flex-shrink: 0;
  border-right: 1px solid #ddd;
}
.grid-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: auto;
  padding: 0.5em;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  gap: 1em;
  flex: 1;
}
.card {
  background: none;
  border: none;
  cursor: pointer;
  text-align: left;
  padding: 0;
  font: inherit;
  position: relative;
}
.card.selected img {
  outline: 3px solid #2a6df4;
  outline-offset: -3px;
}
.card-checkbox {
  position: absolute;
  top: 0.3em;
  left: 0.3em;
  width: 1.1em;
  height: 1.1em;
  z-index: 1;
}
.card img {
  width: 100%;
  height: auto;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
}
.card-title {
  font-weight: 600;
  font-size: 0.85em;
  margin-top: 0.25em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-authors {
  font-size: 0.8em;
  color: #666;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pagination {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1em;
  padding: 0.75em 0;
}
.status,
.error {
  padding: 1em;
}
.error {
  color: #b00020;
}
.add-error,
.add-summary {
  padding: 0 0.5em;
}
.hidden-file-input {
  display: none;
}
.toolbar button.active {
  background: #2a6df4;
  color: #fff;
  border-color: #2a6df4;
}
.fts-enable {
  padding: 1em;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
  align-items: flex-start;
}
.fts-indexing {
  color: #a06a00;
}
.fts-results {
  list-style: none;
  margin: 0;
  padding: 0.5em;
  overflow: auto;
  flex: 1;
}
.fts-result {
  padding: 0.75em;
  border-bottom: 1px solid #eee;
  cursor: pointer;
}
.fts-result:hover {
  background: #f7f7f7;
}
.fts-result-header {
  display: flex;
  align-items: baseline;
  gap: 0.6em;
  flex-wrap: wrap;
}
.fts-result-title {
  font-weight: 600;
}
.fts-result-authors {
  color: #666;
  font-size: 0.9em;
}
.fts-result-formats {
  color: #999;
  font-size: 0.8em;
  text-transform: uppercase;
  margin-left: auto;
}
.fts-snippet {
  margin: 0.4em 0 0;
  font-size: 0.9em;
  color: #444;
}
.fts-snippet mark {
  background: #fff3a0;
  color: inherit;
  padding: 0 0.1em;
}
.bulk-panel {
  padding: 0.75em 1em;
  background: #f7f7f7;
  border-bottom: 1px solid #ddd;
}
.bulk-row {
  display: flex;
  flex-wrap: wrap;
  gap: 1em;
  align-items: flex-end;
}
.bulk-row label {
  display: flex;
  flex-direction: column;
  font-size: 0.85em;
  color: #555;
  gap: 0.2em;
}
.bulk-row input[type="text"],
.bulk-row input:not([type]) {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
.bulk-rating {
  flex-direction: row !important;
  align-items: center;
  gap: 0.4em !important;
}
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.bulk-errors {
  margin: 0.6em 0 0;
  padding: 0;
  list-style: none;
}
.manage-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}
.manage-panel {
  background: #fff;
  border-radius: 6px;
  padding: 1.5em;
  max-width: 560px;
  width: 90%;
  max-height: 85vh;
  overflow: auto;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1.25em;
}
.manage-close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: 1.1em;
  cursor: pointer;
}
.manage-panel h3 {
  margin: 0 0 0.5em;
}
.manage-list {
  list-style: none;
  margin: 0 0 0.75em;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.4em;
}
.manage-list li {
  display: flex;
  align-items: center;
  gap: 0.5em;
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.manage-query {
  color: #666;
  font-size: 0.85em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.manage-remove {
  color: #b00020;
}
.manage-form {
  display: flex;
  gap: 0.5em;
}
.manage-form input {
  flex: 1;
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
</style>
