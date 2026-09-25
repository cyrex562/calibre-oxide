<script setup lang="ts">
import { ref } from "vue";
import { fetchCategories, fetchCategory, renameCategoryItem } from "../library/api";
import { categoryItemToQuery } from "../library/query";
import type { CategoryEntry, CategoryItem } from "../library/types";

const emit = defineEmits<{ select: [query: string, label: string]; "view-note": [field: string, itemName: string]; renamed: [] }>();

const categories = ref<CategoryEntry[]>([]);
const openCategory = ref<string | null>(null);
const items = ref<CategoryItem[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

async function load() {
  try {
    categories.value = await fetchCategories();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
void load();

async function toggle(cat: CategoryEntry) {
  const key = cat.url.split("/").pop() ?? "";
  if (openCategory.value === key) {
    openCategory.value = null;
    items.value = [];
    return;
  }
  openCategory.value = key;
  loading.value = true;
  error.value = null;
  try {
    const page = await fetchCategory(key);
    items.value = page.items;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

function pick(item: CategoryItem) {
  if (!openCategory.value) return;
  emit("select", categoryItemToQuery(openCategory.value, item.name), item.name);
}

// Notes (issue #732) -- every category this browser lists is one of
// the five standard fields notes.rs itself supports
// (authors/tags/series/publisher/languages, see ajax::categories's
// own doc), so no extra allowlist check is needed here: whatever's
// shown in this list is always a valid `field` for the notes routes.
function viewNote(item: CategoryItem) {
  if (!openCategory.value) return;
  emit("view-note", openCategory.value, item.name);
}

// Rename & merge (issue #749) -- backend supports these three real
// categories only (Cache::rename_author/rename_tag/rename_publisher);
// series/languages are real standard categories elsewhere in this
// crate but have no rename method to call, see rename.rs's own doc.
const RENAMEABLE_CATEGORIES = ["authors", "tags", "publisher"];
const renameError = ref<string | null>(null);

async function renameItem(item: CategoryItem) {
  const category = openCategory.value;
  if (!category) return;
  const newName = prompt(`Rename "${item.name}" to:`, item.name);
  if (!newName || newName === item.name) return;
  renameError.value = null;
  try {
    await renameCategoryItem(category, item.name, newName);
    const page = await fetchCategory(category);
    items.value = page.items;
    emit("renamed");
  } catch (e) {
    renameError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <nav class="category-browser">
    <h2 class="cat-heading">Categories</h2>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="renameError" class="error">{{ renameError }}</p>
    <!--
      An empty library still gets a panel that reads as a panel. With
      nothing here at all the column was simply blank, which looks like
      a failed render rather than an empty library.
    -->
    <p v-if="!error && categories.length === 0" class="cat-empty">
      Nothing to browse yet. Authors, series and tags appear here once the library has books.
    </p>
    <ul>
      <li v-for="cat in categories" :key="cat.url">
        <button class="cat-toggle" @click="toggle(cat)">{{ cat.name }}</button>
        <ul v-if="openCategory === cat.url.split('/').pop()" class="items">
          <li v-if="loading">Loading…</li>
          <li v-for="item in items" :key="item.name" class="item-row">
            <button class="item" @click="pick(item)">{{ item.name }} <span class="count">({{ item.count }})</span></button>
            <button v-if="openCategory && RENAMEABLE_CATEGORIES.includes(openCategory)" class="note-btn" title="Rename or merge" @click="renameItem(item)">Rename</button>
            <button class="note-btn" title="View/edit note" @click="viewNote(item)">Note</button>
          </li>
        </ul>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
.cat-heading {
  margin: 0;
  padding: 0.5em 0.6em 0.4em;
  font-size: var(--fs-micro);
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  opacity: 0.6;
}
.cat-empty {
  padding: 0 0.6em;
  font-size: var(--fs-small);
  opacity: 0.7;
  line-height: 1.4;
}
.category-browser {
  overflow: auto;
}
ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.cat-toggle,
.item {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  padding: 0.3em 0.5em;
  cursor: pointer;
  font: inherit;
}
.item-row {
  display: flex;
  align-items: center;
}
.item-row .item {
  flex: 1;
}
.note-btn {
  flex-shrink: 0;
  background: none;
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.15em 0.5em;
  margin-right: 0.4em;
  font-size: var(--fs-small);
  cursor: pointer;
}
.cat-toggle {
  font-weight: 600;
}
.items {
  padding-left: 0.75em;
}
.count {
  color: var(--fg-faint);
  font-size: var(--fs-small);
}
.error {
  color: var(--danger);
  padding: 0.5em;
}
</style>
