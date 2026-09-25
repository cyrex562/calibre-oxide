<script setup lang="ts">
// The categories panel — calibre's tag browser.
//
// This was an accordion: one category open at a time, refetching its
// items on every toggle, with a Rename and a Note button on every
// single row. Opening Tags closed Authors; reopening Authors fetched
// it again; and two extra buttons times N items was the loudest visual
// noise in the window.
//
// It is now a persistent tree. Several categories stay open, expansion
// survives a restart, items are fetched once and cached, and the
// per-item actions moved to a right-click menu.
//
// See docs/UI_DESIGN.md §3.3.

import { computed, ref } from "vue";

import { fetchCategories, fetchCategory, renameCategoryItem } from "../library/api";
import {
  categoriesToReveal, EXPANDED_KEY, keyOf, matchesFilter, parseExpanded,
  serializeExpanded, splitScopedFilter, toggleExpanded, type CategoryKey,
} from "../library/categoryTree";
import { categoryItemToQuery } from "../library/query";
import type { CategoryEntry, CategoryItem } from "../library/types";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";

const emit = defineEmits<{ select: [query: string, label: string]; "view-note": [field: string, itemName: string]; renamed: [] }>();

const categories = ref<CategoryEntry[]>([]);
const error = ref<string | null>(null);

/** Items per category, fetched once and kept. */
const itemsByCategory = ref<Record<CategoryKey, CategoryItem[]>>({});
const loadingCategories = ref<Set<CategoryKey>>(new Set());

const expanded = ref<Set<CategoryKey>>(readExpanded());
const filter = ref("");

function readExpanded(): Set<CategoryKey> {
  try {
    return parseExpanded(localStorage.getItem(EXPANDED_KEY));
  } catch {
    return new Set();
  }
}

function persistExpanded() {
  try {
    localStorage.setItem(EXPANDED_KEY, serializeExpanded(expanded.value));
  } catch {
    // A full or disabled localStorage is not worth failing a render
    // over; expansion simply will not survive a restart.
  }
}

async function load() {
  try {
    categories.value = await fetchCategories();
    // Anything already expanded from a previous session needs its
    // items, or the tree renders open and empty.
    await Promise.all([...expanded.value].map(ensureItems));
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
void load();

/** Fetches a category's items once. Repeat calls are free. */
async function ensureItems(key: CategoryKey) {
  if (itemsByCategory.value[key] || loadingCategories.value.has(key)) return;
  loadingCategories.value = new Set(loadingCategories.value).add(key);
  try {
    const page = await fetchCategory(key);
    itemsByCategory.value = { ...itemsByCategory.value, [key]: page.items };
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    const next = new Set(loadingCategories.value);
    next.delete(key);
    loadingCategories.value = next;
  }
}

async function toggle(cat: CategoryEntry) {
  const key = keyOf(cat.url);
  expanded.value = toggleExpanded(expanded.value, key);
  persistExpanded();
  if (expanded.value.has(key)) await ensureItems(key);
}

function collapseAll() {
  expanded.value = new Set();
  persistExpanded();
}

const knownKeys = computed(() => categories.value.map((c) => keyOf(c.url)));

/**
 * Which categories are drawn open.
 *
 * A filter forces open whatever holds a match, without disturbing the
 * user's own expansion state — clearing the box returns the tree to
 * exactly how they left it.
 */
const revealed = computed(() => categoriesToReveal(filter.value, knownKeys.value, itemsByCategory.value));
const effectivelyOpen = computed(() => new Set([...expanded.value, ...revealed.value]));

/** The filter text to apply within a category, honouring `tags:` scope. */
const scoped = computed(() => splitScopedFilter(filter.value, knownKeys.value));

function visibleItems(key: CategoryKey): CategoryItem[] {
  const items = itemsByCategory.value[key] ?? [];
  if (!filter.value.trim()) return items;
  if (scoped.value.scope && scoped.value.scope !== key) return [];
  return items.filter((item) => matchesFilter(item.name, scoped.value.text));
}

/** Fetch on filtering too, or a match in an unopened category is invisible. */
async function onFilterInput() {
  if (!filter.value.trim()) return;
  await Promise.all(knownKeys.value.map(ensureItems));
}

function pick(key: CategoryKey, item: CategoryItem) {
  emit("select", categoryItemToQuery(key, item.name), item.name);
}

// Rename & merge (issue #749) -- backend supports these three real
// categories only (Cache::rename_author/rename_tag/rename_publisher);
// series/languages are real standard categories elsewhere in this
// crate but have no rename method to call, see rename.rs's own doc.
const RENAMEABLE_CATEGORIES = ["authors", "tags", "publisher"];

// --- per-item menu, replacing two buttons on every row ---

type ItemMenuId = "search" | "rename" | "note";

const itemMenu = ref<{ x: number; y: number; key: CategoryKey; item: CategoryItem } | null>(null);

const itemMenuEntries = computed<ContextMenuEntry<ItemMenuId>[]>(() => {
  const target = itemMenu.value;
  if (!target) return [];
  return [
    { id: "search", label: `Show books in “${target.item.name}”`, enabled: true },
    { id: "rename", label: "Rename or merge…", enabled: RENAMEABLE_CATEGORIES.includes(target.key), startsGroup: true },
    { id: "note", label: "View or edit note…", enabled: true },
  ];
});

function openItemMenu(key: CategoryKey, item: CategoryItem, event: MouseEvent) {
  itemMenu.value = { x: event.clientX, y: event.clientY, key, item };
}

async function onItemMenuChoose(id: ItemMenuId) {
  const target = itemMenu.value;
  itemMenu.value = null;
  if (!target) return;
  if (id === "search") pick(target.key, target.item);
  else if (id === "note") emit("view-note", target.key, target.item.name);
  else await renameItem(target.key, target.item);
}

async function renameItem(key: CategoryKey, item: CategoryItem) {
  // `prompt` is a known tell and is on the list to replace with a real
  // dialog (docs/UI_DESIGN.md §5.5); left here so this rewrite stays
  // about the tree rather than growing a dialog primitive too.
  const newName = prompt(`Rename "${item.name}" to:`, item.name);
  if (!newName || newName === item.name) return;
  error.value = null;
  try {
    await renameCategoryItem(key, item.name, newName);
    const page = await fetchCategory(key);
    itemsByCategory.value = { ...itemsByCategory.value, [key]: page.items };
    emit("renamed");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

/** Icon per standard category, from calibre's own set. */
const CATEGORY_ICONS: Record<string, string> = {
  authors: "user_profile",
  tags: "tags",
  series: "series",
  publisher: "publisher",
  languages: "languages",
  rating: "rating",
};

function iconFor(key: CategoryKey): string | null {
  return CATEGORY_ICONS[key] ?? null;
}

const findOpen = ref(false);
</script>

<template>
  <nav class="category-browser">
    <header class="cat-header">
      <h2>Categories</h2>
      <button type="button" class="cat-tool" :class="{ active: findOpen }" title="Find a category or item" @click="findOpen = !findOpen">⌕</button>
      <button type="button" class="cat-tool" title="Collapse all" @click="collapseAll">▾</button>
    </header>

    <input
      v-if="findOpen"
      v-model="filter"
      class="cat-find"
      type="search"
      placeholder="Find…  (=exact, tags:scope)"
      @input="onFilterInput"
    />

    <p v-if="error" class="error">{{ error }}</p>

    <!--
      An empty library still gets a panel that reads as a panel. With
      nothing here at all the column was simply blank, which looks like
      a failed render rather than an empty library.
    -->
    <p v-if="!error && categories.length === 0" class="cat-empty">
      Nothing to browse yet. Authors, series and tags appear here once the library has books.
    </p>

    <ul class="cat-list">
      <li v-for="cat in categories" :key="cat.url">
        <button
          type="button"
          class="cat-toggle"
          :aria-expanded="effectivelyOpen.has(keyOf(cat.url))"
          @click="toggle(cat)"
        >
          <span class="twisty" aria-hidden="true">{{ effectivelyOpen.has(keyOf(cat.url)) ? "▾" : "▸" }}</span>
          <img v-if="iconFor(keyOf(cat.url))" class="cat-icon" :src="`/icons/${iconFor(keyOf(cat.url))}.png`" alt="" />
          <span class="cat-name">{{ cat.name }}</span>
        </button>

        <ul v-if="effectivelyOpen.has(keyOf(cat.url))" class="items">
          <li v-if="loadingCategories.has(keyOf(cat.url))" class="item-note">Loading…</li>
          <li v-else-if="visibleItems(keyOf(cat.url)).length === 0" class="item-note">
            {{ filter.trim() ? "No matches" : "Empty" }}
          </li>
          <li v-for="item in visibleItems(keyOf(cat.url))" :key="item.name">
            <!--
              One control per row. Rename and Note used to be extra
              buttons on every item; they are in the right-click menu
              now, where they cost nothing until wanted.
            -->
            <button
              type="button"
              class="item"
              :title="`${item.name} — ${item.count} book${item.count === 1 ? '' : 's'}`"
              @click="pick(keyOf(cat.url), item)"
              @contextmenu.prevent="openItemMenu(keyOf(cat.url), item, $event)"
            >
              <span class="item-name">{{ item.name }}</span>
              <span class="count">{{ item.count }}</span>
            </button>
          </li>
        </ul>
      </li>
    </ul>

    <ContextMenu
      v-if="itemMenu"
      :x="itemMenu.x"
      :y="itemMenu.y"
      :entries="itemMenuEntries"
      @choose="onItemMenuChoose"
      @close="itemMenu = null"
    />
  </nav>
</template>

<style scoped>
.category-browser {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  /* Sunken, so the book list reads as the foreground surface. */
  background: var(--bg-sunken);
}

.cat-header {
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  height: 28px;
  padding: 0 var(--sp-2) 0 var(--sp-3);
  border-bottom: 1px solid var(--border);
  background: var(--bg-bar);
  flex-shrink: 0;
}
.cat-header h2 {
  flex: 1;
  margin: 0;
  font-size: var(--fs-small);
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--fg-muted);
}
.cat-tool {
  height: 20px;
  min-width: 20px;
  padding: 0 var(--sp-2);
  border: none;
  background: none;
  color: var(--fg-muted);
  font-size: var(--fs-small);
}
.cat-tool:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.cat-tool.active {
  background: var(--accent-soft);
  color: var(--accent);
}

.cat-find {
  margin: var(--sp-2);
  flex-shrink: 0;
}

.cat-list,
.items {
  list-style: none;
  margin: 0;
  padding: 0;
}
.cat-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.cat-toggle,
.item {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  width: 100%;
  height: 22px;
  padding: 0 var(--sp-3);
  border: none;
  border-radius: 0;
  background: none;
  text-align: left;
  font-size: var(--fs-body);
}
.cat-toggle:hover,
.item:hover {
  background: var(--bg-hover);
}
.cat-toggle {
  font-weight: 600;
}
.twisty {
  flex: 0 0 10px;
  font-size: var(--fs-micro);
  color: var(--fg-faint);
}
.cat-icon {
  width: 16px;
  height: 16px;
  filter: var(--icon-filter);
  flex-shrink: 0;
}
.cat-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* One indent level, matching the twisty plus icon above. */
.item {
  padding-left: calc(var(--sp-3) + 12px);
}
.item-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.count {
  font-size: var(--fs-micro);
  color: var(--fg-faint);
  font-variant-numeric: tabular-nums;
}

.item-note {
  padding: var(--sp-1) var(--sp-3) var(--sp-1) calc(var(--sp-3) + 12px);
  font-size: var(--fs-small);
  color: var(--fg-faint);
}

.cat-empty {
  padding: var(--sp-4) var(--sp-3);
  font-size: var(--fs-small);
  color: var(--fg-faint);
  line-height: 1.4;
  margin: 0;
}

.error {
  padding: var(--sp-2) var(--sp-3);
  color: var(--danger);
  font-size: var(--fs-small);
  margin: 0;
}
</style>
