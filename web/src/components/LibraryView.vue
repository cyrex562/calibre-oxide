<script setup lang="ts">
import { computed, ref, watch } from "vue";
import CategoryBrowser from "./CategoryBrowser.vue";
import BookDetailsPanel from "./BookDetailsPanel.vue";
import { fetchBooks, fetchFieldMetadata, fetchVirtualLibraries, search } from "../library/api";
import type { BookSummary } from "../library/types";

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

watch([activeQuery, sort, sortOrder, vl, offset], runSearch, { immediate: true });

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
</script>

<template>
  <div class="library">
    <header class="toolbar">
      <form class="search" @submit.prevent="submitSearch">
        <input v-model="queryText" type="search" placeholder="Search…" />
        <button type="submit">Search</button>
      </form>

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
    </header>

    <div class="body">
      <CategoryBrowser class="sidebar" @select="onCategorySelect" />

      <main class="grid-area">
        <p v-if="error" class="error">{{ error }}</p>
        <p v-else-if="loading" class="status">Loading…</p>
        <p v-else-if="books.length === 0" class="status">No books found.</p>

        <div class="grid">
          <button v-for="book in books" :key="book.id" class="card" @click="selectedBookId = book.id">
            <img :src="book.thumbnail" :alt="book.title" loading="lazy" />
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

    <BookDetailsPanel v-if="selectedBookId !== null" :book-id="selectedBookId" @close="selectedBookId = null" />
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
</style>
