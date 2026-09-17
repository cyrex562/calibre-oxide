<script setup lang="ts">
import { computed, ref } from "vue";
import { blobToDataUrl, coverProxyUrl, fetchCoverProxyBlob, searchMetadataOnline, setFields, type MetadataCandidate } from "../library/api";
import type { BookFieldChanges, BookSummary } from "../library/types";

const props = defineProps<{ bookId: number; book: BookSummary | null }>();
const emit = defineEmits<{ close: []; updated: [] }>();

// Real upstream calibre's own "identifiers" field is a single
// comma-joined "key:value,key:value" string (Cache::field_for's own
// storage format, not an object) -- parsed here just to prefill an
// ISBN search field, not stored back in that shape anywhere.
function parseIdentifiers(raw: unknown): Record<string, string> {
  if (typeof raw !== "string" || raw.trim() === "") return {};
  const out: Record<string, string> = {};
  for (const part of raw.split(",")) {
    const idx = part.indexOf(":");
    if (idx <= 0) continue;
    out[part.slice(0, idx).trim()] = part.slice(idx + 1).trim();
  }
  return out;
}

const searchTitle = ref(props.book?.title ?? "");
const searchAuthors = ref((props.book?.authors ?? []).join(" "));
const searchIsbn = ref(parseIdentifiers(props.book?.identifiers).isbn ?? "");

const searching = ref(false);
const searchError = ref<string | null>(null);
const candidates = ref<MetadataCandidate[]>([]);
const sourceErrors = ref<string[]>([]);
const searched = ref(false);

async function runSearch() {
  searching.value = true;
  searchError.value = null;
  searched.value = false;
  try {
    const result = await searchMetadataOnline({
      title: searchTitle.value.trim() || undefined,
      authors: searchAuthors.value.trim() || undefined,
      isbn: searchIsbn.value.trim() || undefined,
    });
    candidates.value = result.candidates;
    sourceErrors.value = result.source_errors;
    searched.value = true;
  } catch (e) {
    searchError.value = e instanceof Error ? e.message : String(e);
  } finally {
    searching.value = false;
  }
}
void runSearch();

type PickerField = "title" | "authors" | "description" | "publisher" | "pubdate" | "tags" | "identifiers" | "language" | "cover";

const PICKER_FIELDS: { field: PickerField; label: string }[] = [
  { field: "title", label: "Title" },
  { field: "authors", label: "Authors" },
  { field: "cover", label: "Cover" },
  { field: "description", label: "Description" },
  { field: "publisher", label: "Publisher" },
  { field: "pubdate", label: "Published" },
  { field: "tags", label: "Tags" },
  { field: "language", label: "Language" },
  { field: "identifiers", label: "Identifiers" },
];

const pickerCandidate = ref<MetadataCandidate | null>(null);
const fieldsToApply = ref<Record<PickerField, boolean>>({} as Record<PickerField, boolean>);
const applying = ref(false);
const applyError = ref<string | null>(null);
const applyDone = ref(false);

function candidateHasField(c: MetadataCandidate, field: PickerField): boolean {
  switch (field) {
    case "title":
      return !!c.title;
    case "authors":
      return c.authors.length > 0;
    case "cover":
      return !!c.cover_url;
    case "description":
      return !!c.description;
    case "publisher":
      return !!c.publisher;
    case "pubdate":
      return !!c.pubdate;
    case "tags":
      return c.tags.length > 0;
    case "language":
      return !!c.language;
    case "identifiers":
      return Object.keys(c.identifiers).length > 0;
  }
}

function candidateFieldText(c: MetadataCandidate, field: PickerField): string {
  switch (field) {
    case "title":
      return c.title ?? "";
    case "authors":
      return c.authors.join(", ");
    case "cover":
      return c.cover_url ? "(image)" : "";
    case "description":
      return c.description ?? "";
    case "publisher":
      return c.publisher ?? "";
    case "pubdate":
      return c.pubdate ?? "";
    case "tags":
      return c.tags.join(", ");
    case "language":
      return c.language ?? "";
    case "identifiers":
      return Object.entries(c.identifiers).map(([k, v]) => `${k}:${v}`).join(", ");
  }
}

function currentFieldText(field: PickerField): string {
  const b = props.book;
  if (!b) return "";
  switch (field) {
    case "title":
      return b.title;
    case "authors":
      return (b.authors ?? []).join(", ");
    case "cover":
      return "(current cover)";
    case "description":
      return typeof b.comments === "string" ? b.comments : "";
    case "publisher":
      return typeof b.publisher === "string" ? b.publisher : "";
    case "pubdate":
      return typeof b.pubdate === "string" ? b.pubdate : "";
    case "tags":
      return (b.tags ?? []).join(", ");
    case "language": {
      const langs = b.languages;
      return Array.isArray(langs) ? langs.join(", ") : "";
    }
    case "identifiers":
      return typeof b.identifiers === "string" ? b.identifiers : "";
  }
}

function openPicker(c: MetadataCandidate) {
  pickerCandidate.value = c;
  applyError.value = null;
  applyDone.value = false;
  const initial = {} as Record<PickerField, boolean>;
  for (const { field } of PICKER_FIELDS) {
    initial[field] = candidateHasField(c, field);
  }
  fieldsToApply.value = initial;
}

const pickerRows = computed(() => {
  const c = pickerCandidate.value;
  if (!c) return [];
  return PICKER_FIELDS.filter(({ field }) => candidateHasField(c, field));
});

async function applyPicked() {
  const c = pickerCandidate.value;
  if (!c) return;
  applying.value = true;
  applyError.value = null;
  try {
    const changes: BookFieldChanges = {};
    if (fieldsToApply.value.title && c.title) changes.title = c.title;
    if (fieldsToApply.value.authors && c.authors.length > 0) changes.authors = c.authors;
    if (fieldsToApply.value.description && c.description) changes.comments = c.description;
    if (fieldsToApply.value.publisher && c.publisher) changes.publisher = c.publisher;
    if (fieldsToApply.value.pubdate && c.pubdate) changes.pubdate = c.pubdate;
    if (fieldsToApply.value.tags && c.tags.length > 0) changes.tags = c.tags;
    if (fieldsToApply.value.language && c.language) changes.languages = [c.language];
    if (fieldsToApply.value.identifiers && Object.keys(c.identifiers).length > 0) changes.identifiers = c.identifiers;
    if (fieldsToApply.value.cover && c.cover_url) {
      const blob = await fetchCoverProxyBlob(c.cover_url);
      changes.cover = await blobToDataUrl(blob);
    }
    await setFields(props.bookId, changes);
    applyDone.value = true;
    emit("updated");
  } catch (e) {
    applyError.value = e instanceof Error ? e.message : String(e);
  } finally {
    applying.value = false;
  }
}
</script>

<template>
  <div class="backdrop">
    <div class="panel">
      <button class="close" @click="emit('close')">✕</button>
      <h3>Fetch metadata online</h3>

      <template v-if="!pickerCandidate">
        <form class="search-form" @submit.prevent="runSearch">
          <label class="field">
            Title
            <input v-model="searchTitle" type="text" />
          </label>
          <label class="field">
            Authors
            <input v-model="searchAuthors" type="text" />
          </label>
          <label class="field">
            ISBN
            <input v-model="searchIsbn" type="text" placeholder="overrides title/authors when set" />
          </label>
          <button type="submit" :disabled="searching">{{ searching ? "Searching…" : "Search" }}</button>
        </form>

        <p v-if="searchError" class="error">{{ searchError }}</p>
        <p v-if="sourceErrors.length" class="hint">Some sources didn't respond: {{ sourceErrors.join("; ") }}</p>

        <ul v-if="searched" class="candidates">
          <li v-if="candidates.length === 0" class="hint">No results.</li>
          <li v-for="(c, i) in candidates" :key="i" class="candidate">
            <img v-if="c.cover_url" class="thumb" :src="coverProxyUrl(c.cover_url)" alt="" />
            <div class="candidate-body">
              <div class="candidate-title">{{ c.title ?? "(untitled)" }}</div>
              <div class="candidate-authors">{{ c.authors.join(", ") }}</div>
              <div class="candidate-source">{{ c.source }}</div>
            </div>
            <button type="button" @click="openPicker(c)">Review & apply…</button>
          </li>
        </ul>
      </template>

      <template v-else>
        <button type="button" class="back" @click="pickerCandidate = null">← Back to results</button>
        <p class="hint">Choose which fields to apply from {{ pickerCandidate.source }}.</p>
        <table class="picker">
          <thead>
            <tr>
              <th></th>
              <th>Field</th>
              <th>Current</th>
              <th>{{ pickerCandidate.source }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="{ field, label } in pickerRows" :key="field">
              <td><input v-model="fieldsToApply[field]" type="checkbox" /></td>
              <td>{{ label }}</td>
              <td class="value">{{ currentFieldText(field) }}</td>
              <td class="value">
                <img v-if="field === 'cover'" class="thumb" :src="coverProxyUrl(pickerCandidate.cover_url!)" alt="" />
                <template v-else>{{ candidateFieldText(pickerCandidate, field) }}</template>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="applyError" class="error">{{ applyError }}</p>
        <p v-if="applyDone" class="saved">Applied.</p>
        <button type="button" :disabled="applying" @click="applyPicked">{{ applying ? "Applying…" : "Apply selected fields" }}</button>
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
  width: 90%;
  max-width: 720px;
  max-height: 80vh;
  overflow: auto;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 0.75em;
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
h3 {
  margin: 0;
}
.hint {
  color: #888;
  font-size: 0.85em;
  margin: 0;
}
.search-form {
  display: flex;
  gap: 0.75em;
  align-items: flex-end;
  flex-wrap: wrap;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.25em;
  font-size: 0.85em;
}
.candidates {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}
.candidate {
  display: flex;
  align-items: center;
  gap: 0.75em;
  border: 1px solid #ddd;
  border-radius: 4px;
  padding: 0.5em;
}
.thumb {
  width: 40px;
  height: 58px;
  object-fit: cover;
  flex-shrink: 0;
  background: #eee;
}
.candidate-body {
  flex: 1;
  min-width: 0;
}
.candidate-title {
  font-weight: 600;
}
.candidate-authors,
.candidate-source {
  font-size: 0.85em;
  color: #666;
}
.back {
  align-self: flex-start;
  border: none;
  background: none;
  cursor: pointer;
  color: #2563eb;
  padding: 0;
}
.picker {
  border-collapse: collapse;
  width: 100%;
  font-size: 0.9em;
}
.picker th,
.picker td {
  border: 1px solid #ddd;
  padding: 0.4em 0.5em;
  text-align: left;
  vertical-align: top;
}
.picker .value {
  max-width: 220px;
  overflow-wrap: anywhere;
}
.error {
  color: #b00020;
}
.saved {
  color: #1b7f3a;
}
</style>
