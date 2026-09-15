<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { fetchBook, setCover, setFields } from "../library/api";
import type { BookFieldChanges, BookSummary } from "../library/types";

const props = defineProps<{ bookId: number }>();
const emit = defineEmits<{ close: []; updated: [] }>();
const router = useRouter();

const book = ref<BookSummary | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);

const editing = ref(false);
const saving = ref(false);
const saveError = ref<string | null>(null);
const editTitle = ref("");
const editAuthors = ref("");
const editSeries = ref("");
const editSeriesIndex = ref("");
const editTags = ref("");
const editRating = ref(0);
const coverInput = ref<HTMLInputElement | null>(null);

function startEditing() {
  const b = book.value;
  if (!b) return;
  editTitle.value = b.title;
  editAuthors.value = (b.authors ?? []).join(" & ");
  editSeries.value = b.series ?? "";
  editSeriesIndex.value = b.series_index != null ? String(b.series_index) : "";
  editTags.value = (b.tags ?? []).join(", ");
  editRating.value = b.rating ?? 0;
  saveError.value = null;
  editing.value = true;
}

async function saveEdits() {
  saving.value = true;
  saveError.value = null;
  try {
    const changes: BookFieldChanges = {
      title: editTitle.value,
      authors: editAuthors.value.split("&").map((a) => a.trim()).filter(Boolean),
      series: editSeries.value,
      tags: editTags.value.split(",").map((t) => t.trim()).filter(Boolean),
      rating: editRating.value,
    };
    if (editSeriesIndex.value.trim() !== "") {
      const idx = Number(editSeriesIndex.value);
      if (!Number.isNaN(idx)) changes.series_index = idx;
    }
    book.value = await setFields(props.bookId, changes);
    editing.value = false;
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

async function replaceCover(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  saving.value = true;
  saveError.value = null;
  try {
    await setCover(props.bookId, file);
    book.value = await fetchBook(props.bookId);
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
    if (coverInput.value) coverInput.value.value = "";
  }
}

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
  editing.value = false;
  try {
    book.value = await fetchBook(id);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
watch(() => props.bookId, load, { immediate: true });

// The cover/thumbnail URLs are stable paths (`/get/cover/{id}`) --
// after replacing the cover the browser would otherwise keep showing
// the cached old image. Bust the cache with a query param that
// changes only when the book data itself was just reloaded.
const coverVersion = ref(0);
watch(book, () => coverVersion.value++);
const coverSrc = computed(() => (book.value ? `${book.value.thumbnail}?v=${coverVersion.value}` : ""));

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
      <template v-else-if="book && !editing">
        <div class="header">
          <img :src="coverSrc" alt="" class="cover" />
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
          <button class="edit" @click="startEditing">Edit metadata</button>
        </div>
      </template>

      <form v-else-if="book && editing" class="edit-form" @submit.prevent="saveEdits">
        <div class="header">
          <div class="cover-edit">
            <img :src="coverSrc" alt="" class="cover" />
            <button type="button" @click="coverInput?.click()">Replace cover…</button>
            <input ref="coverInput" type="file" accept="image/jpeg,image/png" class="hidden-file-input" @change="replaceCover" />
          </div>
          <div class="fields">
            <label>Title <input v-model="editTitle" required /></label>
            <label>Authors <input v-model="editAuthors" placeholder="Jane Doe & John Smith" /></label>
            <label>Series <input v-model="editSeries" /></label>
            <label>Series index <input v-model="editSeriesIndex" type="number" step="0.1" /></label>
            <label>Tags <input v-model="editTags" placeholder="scifi, classic" /></label>
            <label>Rating <input v-model.number="editRating" type="number" min="0" max="5" step="1" /></label>
          </div>
        </div>

        <p v-if="saveError" class="error">{{ saveError }}</p>

        <div class="formats">
          <button type="submit" class="read" :disabled="saving">{{ saving ? "Saving…" : "Save" }}</button>
          <button type="button" @click="editing = false" :disabled="saving">Cancel</button>
        </div>
      </form>
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
.edit {
  background: none;
  border: 1px solid #ccc;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.edit-form .header {
  align-items: flex-start;
}
.cover-edit {
  display: flex;
  flex-direction: column;
  gap: 0.4em;
  width: 90px;
  flex-shrink: 0;
}
.hidden-file-input {
  display: none;
}
.fields {
  display: flex;
  flex-direction: column;
  gap: 0.5em;
  flex: 1;
}
.fields label {
  display: flex;
  flex-direction: column;
  font-size: 0.85em;
  color: #555;
  gap: 0.2em;
}
.fields input {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
</style>
