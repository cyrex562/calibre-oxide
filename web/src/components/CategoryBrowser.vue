<script setup lang="ts">
import { ref } from "vue";
import { fetchCategories, fetchCategory } from "../library/api";
import { categoryItemToQuery } from "../library/query";
import type { CategoryEntry, CategoryItem } from "../library/types";

const emit = defineEmits<{ select: [query: string, label: string]; "view-note": [field: string, itemName: string] }>();

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
</script>

<template>
  <nav class="category-browser">
    <p v-if="error" class="error">{{ error }}</p>
    <ul>
      <li v-for="cat in categories" :key="cat.url">
        <button class="cat-toggle" @click="toggle(cat)">{{ cat.name }}</button>
        <ul v-if="openCategory === cat.url.split('/').pop()" class="items">
          <li v-if="loading">Loading…</li>
          <li v-for="item in items" :key="item.name" class="item-row">
            <button class="item" @click="pick(item)">{{ item.name }} <span class="count">({{ item.count }})</span></button>
            <button class="note-btn" title="View/edit note" @click="viewNote(item)">Note</button>
          </li>
        </ul>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
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
  border: 1px solid #ccc;
  border-radius: 4px;
  padding: 0.15em 0.5em;
  margin-right: 0.4em;
  font-size: 0.8em;
  cursor: pointer;
}
.cat-toggle {
  font-weight: 600;
}
.items {
  padding-left: 0.75em;
}
.count {
  color: #888;
  font-size: 0.85em;
}
.error {
  color: #b00020;
  padding: 0.5em;
}
</style>
