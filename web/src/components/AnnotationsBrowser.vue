<script setup lang="ts">
// Library-wide annotation browser (issue 1.8 of the #816 epic).
//
// Per-book annotations were always reachable, but nothing could
// answer "what have I highlighted across this whole library" -- which
// is the question that makes keeping highlights worthwhile. The
// engine behind it (`calibre_db::annotations::all_annotations`) had
// to be ported for this; despite what that module's doc implied, it
// did not exist.

import { computed, onMounted, ref } from "vue";

import { fetchAllAnnotations, type LibraryAnnotation } from "../library/api";

const emit = defineEmits<{ close: []; "open-book": [bookId: number] }>();

const annotations = ref<LibraryAnnotation[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const typeFilter = ref<"" | "highlight" | "bookmark">("");
const search = ref("");

async function load() {
  loading.value = true;
  error.value = null;
  try {
    annotations.value = await fetchAllAnnotations(typeFilter.value ? { type: typeFilter.value } : {});
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

// Filtering the loaded page in the browser rather than round-tripping:
// the server already caps the response, and typing should not fire a
// request per keystroke.
const shown = computed(() => {
  const q = search.value.trim().toLowerCase();
  if (!q) return annotations.value;
  return annotations.value.filter((a) => a.text.toLowerCase().includes(q) || a.title.toLowerCase().includes(q));
});

function formatDate(ts: string | null): string {
  if (!ts) return "";
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString();
}

onMounted(load);
</script>

<template>
  <div class="manage-backdrop" @click.self="emit('close')">
    <div class="manage-panel annotations-panel">
      <h3>Annotations</h3>

      <div class="annot-controls">
        <select v-model="typeFilter" :disabled="loading" @change="load">
          <option value="">All types</option>
          <option value="highlight">Highlights</option>
          <option value="bookmark">Bookmarks</option>
        </select>
        <input v-model="search" type="search" placeholder="Filter…" :disabled="loading" />
        <button type="button" :disabled="loading" @click="load">{{ loading ? "Loading…" : "Refresh" }}</button>
        <button type="button" @click="emit('close')">Close</button>
      </div>

      <p v-if="error" class="error">{{ error }}</p>
      <p v-else-if="loading" class="status">Loading…</p>
      <p v-else-if="annotations.length === 0" class="status">No annotations in this library yet.</p>
      <p v-else-if="shown.length === 0" class="status">Nothing matches that filter.</p>

      <ul v-if="!loading && shown.length" class="annot-list">
        <li v-for="a in shown" :key="a.id" class="annot-row">
          <button type="button" class="annot-book" :title="`Open ${a.title}`" @click="emit('open-book', a.book_id)">{{ a.title }}</button>
          <span class="annot-type">{{ a.type }}</span>
          <span class="annot-text">{{ a.text || "(no text)" }}</span>
          <span class="annot-date">{{ formatDate(a.timestamp) }}</span>
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.annotations-panel {
  min-width: min(820px, 94vw);
}
.annot-controls {
  display: flex;
  gap: 0.4rem;
  margin-bottom: 0.6rem;
  flex-wrap: wrap;
}
.annot-controls input {
  flex: 1;
  min-width: 10ch;
}
.annot-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 60vh;
  overflow-y: auto;
}
.annot-row {
  display: grid;
  grid-template-columns: minmax(10ch, 1.2fr) 6rem minmax(12ch, 3fr) 7rem;
  gap: 0.6rem;
  align-items: baseline;
  padding: 0.35rem 0;
  border-bottom: 1px solid var(--border);
  font-size: var(--fs-small);
}
.annot-book {
  all: unset;
  cursor: pointer;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.annot-book:hover {
  text-decoration: underline;
}
.annot-type {
  opacity: 0.6;
  font-size: var(--fs-small);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.annot-text {
  overflow: hidden;
  text-overflow: ellipsis;
}
.annot-date {
  opacity: 0.6;
  font-variant-numeric: tabular-nums;
  text-align: right;
}
</style>
