<script setup lang="ts">
import { ref } from "vue";
import { fetchCategories, fetchCategory } from "../library/api";
import { categoryItemToQuery } from "../library/query";
import type { CategoryEntry, CategoryItem } from "../library/types";

const emit = defineEmits<{ select: [query: string, label: string] }>();

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
</script>

<template>
  <nav class="category-browser">
    <p v-if="error" class="error">{{ error }}</p>
    <ul>
      <li v-for="cat in categories" :key="cat.url">
        <button class="cat-toggle" @click="toggle(cat)">{{ cat.name }}</button>
        <ul v-if="openCategory === cat.url.split('/').pop()" class="items">
          <li v-if="loading">Loading…</li>
          <li v-for="item in items" :key="item.name">
            <button class="item" @click="pick(item)">{{ item.name }} <span class="count">({{ item.count }})</span></button>
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
