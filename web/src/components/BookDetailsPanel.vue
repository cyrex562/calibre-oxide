<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { fetchBook } from "../library/api";
import type { BookSummary } from "../library/types";

const props = defineProps<{ bookId: number }>();
const emit = defineEmits<{ close: [] }>();
const router = useRouter();

const book = ref<BookSummary | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);

// Only formats the reader MVP (#499) actually round-trips through
// render_book are offered a "Read" link -- other formats still get a
// plain download link.
const READABLE_FORMATS = ["epub", "kepub"];

const readableFormat = computed(() => book.value?.formats.find((f) => READABLE_FORMATS.includes(f)) ?? null);

const formatLinks = computed<[string, string][]>(() => {
  const b = book.value;
  if (!b) return [];
  const links: [string, string][] = [];
  if (b.main_format) links.push(...(Object.entries(b.main_format) as [string, string][]));
  links.push(...(Object.entries(b.other_formats) as [string, string][]));
  return links;
});

async function load(id: number) {
  loading.value = true;
  error.value = null;
  book.value = null;
  try {
    book.value = await fetchBook(id);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
watch(() => props.bookId, load, { immediate: true });

function read() {
  if (!readableFormat.value) return;
  void router.push({ name: "read", params: { bookId: String(props.bookId), fmt: readableFormat.value } });
}
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="panel">
      <button class="close" @click="emit('close')">✕</button>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else-if="book">
        <div class="header">
          <img :src="book.thumbnail" alt="" class="cover" />
          <div>
            <h2>{{ book.title }}</h2>
            <p class="authors">{{ (book.authors ?? []).join(" & ") }}</p>
            <p v-if="book.series" class="series">{{ book.series }} #{{ book.series_index }}</p>
            <p v-if="book.rating" class="rating">{{ "★".repeat(Math.round(book.rating)) }}</p>
          </div>
        </div>

        <p v-if="(book.tags ?? []).length" class="tags">{{ (book.tags ?? []).join(", ") }}</p>

        <div class="formats">
          <button v-if="readableFormat" class="read" @click="read">Read ({{ readableFormat.toUpperCase() }})</button>
          <a v-for="[fmt, url] in formatLinks" :key="fmt" :href="url" class="download"> Download {{ fmt.toUpperCase() }} </a>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}
.panel {
  background: #fff;
  border-radius: 6px;
  padding: 1.5em;
  max-width: 520px;
  width: 90%;
  max-height: 85vh;
  overflow: auto;
  position: relative;
}
.close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: 1.1em;
  cursor: pointer;
}
.header {
  display: flex;
  gap: 1em;
}
.cover {
  width: 90px;
  height: auto;
  flex-shrink: 0;
}
.authors {
  color: #555;
}
.tags {
  color: #888;
  font-size: 0.9em;
}
.formats {
  margin-top: 1em;
  display: flex;
  gap: 0.5em;
  flex-wrap: wrap;
}
.read {
  background: #2a6df4;
  color: #fff;
  border: none;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.download {
  border: 1px solid #ccc;
  padding: 0.5em 1em;
  border-radius: 4px;
  text-decoration: none;
  color: inherit;
}
.error {
  color: #b00020;
}
</style>
