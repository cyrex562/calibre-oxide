<script setup lang="ts">
import { computed, ref, watch } from "vue";
import CategoryBrowser from "./CategoryBrowser.vue";
import BookDetailsPanel from "./BookDetailsPanel.vue";
import { addBook, fetchBooks, fetchFieldMetadata, fetchVirtualLibraries, ftsSearch, ftsSnippets, search, setFtsEnabled } from "../library/api";
import { parseSnippetSegments } from "../library/snippets";
import type { BookSummary, FtsSnippet } from "../library/types";

const PAGE_SIZE = 24;

const queryText = ref("");
const activeQuery = ref(""); // committed query -- what's actually sent, vs. the input box's live text
const sort = ref("timestamp");
const sortOrder = ref<"asc" | "desc">("desc");
const vl = ref("");
const offset = ref(0);

const sortableFields = ref<[string, string][]>([]);
const virtualLibraries = ref<Record<string, string>>({});

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
    const [fm, vls] = await Promise.all([fetchFieldMetadata(), fetchVirtualLibraries()]);
    sortableFields.value = fm.sortable_fields;
    virtualLibraries.value = vls;
  } catch (e) {
    // Non-fatal -- the grid itself still works with default sort/no vl.
    console.error("failed to load field metadata / virtual libraries", e);
  }
}
void loadMetadata();

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

async function addBookFile(file: File, addDuplicates: boolean): Promise<void> {
  const result = await addBook(file, addDuplicates);
  if (result.duplicates && result.duplicates.length > 0 && result.book_id === undefined) {
    const names = result.duplicates.map((d) => `${d.title} (${d.authors.join(" & ")})`).join(", ");
    if (confirm(`A book with the same title/author already exists: ${names}. Add anyway?`)) {
      await addBookFile(file, true);
    }
    return;
  }
  cacheBust.value++;
  await runSearch();
}

async function onAddFileSelected(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  adding.value = true;
  addError.value = null;
  try {
    await addBookFile(file, false);
  } catch (err) {
    addError.value = err instanceof Error ? err.message : String(err);
  } finally {
    adding.value = false;
    if (addInput.value) addInput.value.value = "";
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
      </template>

      <button type="button" :disabled="adding" @click="addInput?.click()">{{ adding ? "Adding…" : "Add Book…" }}</button>
      <input ref="addInput" type="file" class="hidden-file-input" @change="onAddFileSelected" />
    </header>

    <p v-if="addError" class="error add-error">{{ addError }}</p>

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
          <button v-for="book in books" :key="book.id" class="card" @click="selectedBookId = book.id">
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
.add-error {
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
</style>
