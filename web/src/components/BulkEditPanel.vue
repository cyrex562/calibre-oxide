<script setup lang="ts">
// The bulk-edit panel: the shell over `useBulkEdit` (../library/useBulkEdit.ts),
// which holds what is edited and what Apply does. It sits above the list, not in
// a dialog, so the selection it acts on stays in view.

import { completionForCustom } from "../library/fieldValues";
import { REPLACEABLE_FIELDS, REPLACEABLE_FIELD_LABELS } from "../library/bulkEdit";
import type { BulkEdit } from "../library/useBulkEdit";
import AutocompleteInput from "./AutocompleteInput.vue";

defineProps<{ bulk: BulkEdit; count: number }>();

// An example is shown as one ("e.g. …"), so it is not mistaken for a value already typed.
const EXAMPLE = { tags: "e.g. scifi, classic", authors: "e.g. Ann Lee, Bo Fox", languages: "e.g. eng, fra" };
</script>

<template>
<div class="bulk-panel" role="region" aria-label="Bulk edit">
  <h3 class="bulk-title" data-testid="bulk-title">Bulk edit — {{ count.toLocaleString("en-US") }} {{ count === 1 ? "book" : "books" }}</h3>
  <p class="bulk-hint">Only the fields you fill in are changed. You can undo the edit right after.</p>
  <div class="bulk-grid">
    <label class="bulk-field">
      <span>Tags</span>
      <div class="bulk-combo">
        <select v-model="bulk.spec.tags!.mode" :disabled="bulk.busy">
          <option value="add">Add</option>
          <option value="remove">Remove</option>
          <option value="replace">Replace with</option>
        </select>
        <AutocompleteInput v-model="bulk.spec.tags!.value" field="tags" separator="," :placeholder="EXAMPLE.tags" :disabled="bulk.busy" />
      </div>
    </label>

    <label class="bulk-field">
      <span>Authors</span>
      <div class="bulk-combo">
        <select v-model="bulk.spec.authors!.mode" :disabled="bulk.busy">
          <option value="add">Add</option>
          <option value="remove">Remove</option>
          <option value="replace">Replace with</option>
        </select>
        <AutocompleteInput v-model="bulk.spec.authors!.value" field="authors" separator="," :placeholder="EXAMPLE.authors" :disabled="bulk.busy" />
      </div>
    </label>

    <label class="bulk-field">
      <span>Languages</span>
      <div class="bulk-combo">
        <select v-model="bulk.spec.languages!.mode" :disabled="bulk.busy">
          <option value="add">Add</option>
          <option value="remove">Remove</option>
          <option value="replace">Replace with</option>
        </select>
        <input v-model="bulk.spec.languages!.value" :placeholder="EXAMPLE.languages" :disabled="bulk.busy" />
      </div>
    </label>

    <label class="bulk-field"><span>Publisher</span><AutocompleteInput :model-value="bulk.spec.publisher ?? ''" @update:model-value="bulk.spec.publisher = $event" field="publisher" :disabled="bulk.busy" /></label>
    <label class="bulk-field"><span>Series</span><AutocompleteInput :model-value="bulk.spec.series ?? ''" @update:model-value="bulk.spec.series = $event" field="series" :disabled="bulk.busy" /></label>
    <label class="bulk-field"><span>Published</span><input v-model="bulk.spec.pubdate" type="date" :disabled="bulk.busy" /></label>
    <label class="bulk-field"><span>Comments</span><input v-model="bulk.spec.comments" :disabled="bulk.busy" /></label>

    <label class="bulk-field bulk-optional">
      <span><input type="checkbox" v-model="bulk.applySeriesIndex" :disabled="bulk.busy" /> Series index</span>
      <input type="number" v-model.number="bulk.seriesIndex" step="1" :disabled="bulk.busy || !bulk.applySeriesIndex" />
    </label>

    <label class="bulk-field bulk-optional">
      <span><input type="checkbox" v-model="bulk.applyRating" :disabled="bulk.busy" /> Rating</span>
      <input type="number" v-model.number="bulk.rating" min="0" max="5" step="1" :disabled="bulk.busy || !bulk.applyRating" />
    </label>

    <label v-for="column in bulk.customColumns" :key="column.label" class="bulk-field">
      <span>{{ column.name || column.label }}</span>
      <AutocompleteInput v-if="completionForCustom(column)" :model-value="bulk.spec.custom![column.label] ?? ''" :field="column.key" :separator="completionForCustom(column)!.separator" :disabled="bulk.busy" @update:model-value="bulk.spec.custom![column.label] = $event" />
      <input v-else :value="bulk.spec.custom![column.label] ?? ''" :disabled="bulk.busy" @input="bulk.spec.custom![column.label] = ($event.target as HTMLInputElement).value" />
    </label>
  </div>

  <fieldset class="bulk-replace">
    <legend>Search and replace</legend>
    <div class="bulk-combo">
      <select v-model="bulk.spec.searchReplace!.field" aria-label="Field to search and replace in" :disabled="bulk.busy">
        <option v-for="f in REPLACEABLE_FIELDS" :key="f" :value="f">{{ REPLACEABLE_FIELD_LABELS[f] }}</option>
      </select>
      <input v-model="bulk.spec.searchReplace!.find" placeholder="find…" aria-label="Text to find" :disabled="bulk.busy" />
      <input v-model="bulk.spec.searchReplace!.replace" placeholder="replace with…" aria-label="Text to replace it with" :disabled="bulk.busy" />
      <label class="bulk-regex"><input type="checkbox" v-model="bulk.spec.searchReplace!.useRegex" :disabled="bulk.busy" /> Regex</label>
    </div>
  </fieldset>

  <div class="bulk-actions">
    <button type="button" class="primary" :disabled="bulk.busy" @click="bulk.run">
      {{ bulk.progress ? `Applying… ${bulk.progress.done.toLocaleString("en-US")} of ${bulk.progress.total.toLocaleString("en-US")}` : bulk.busy ? "Applying…" : `Apply to ${count} book(s)` }}
    </button>
    <button v-if="bulk.busy" type="button" data-testid="bulk-cancel" @click="bulk.cancel">Cancel</button>
    <button type="button" :disabled="bulk.busy" @click="bulk.open = false">Close</button>
  </div>
  <p v-if="bulk.summary" class="status">{{ bulk.summary }}</p>
  <ul v-if="bulk.errors.length" class="bulk-errors">
    <li v-for="(err, i) in bulk.errors" :key="i" class="error">{{ err }}</li>
  </ul>
</div>
</template>

<style scoped>
.bulk-panel {
  padding: 0.75em 1em;
  background: var(--bg-sunken);
  border-bottom: 1px solid var(--border);
}
.bulk-title {
  margin: 0 0 0.15em;
  font-size: var(--fs-body);
}
.bulk-hint {
  margin: 0 0 0.6em;
  color: var(--fg-muted);
  font-size: var(--fs-small);
}
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.bulk-errors {
  margin: 0.6em 0 0;
  padding: 0;
  list-style: none;
}
.status,
.error {
  padding: 1em;
}
.error {
  color: var(--danger);
}
/* Expanded bulk edit (#1.6). */
.bulk-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 0.5rem 1rem;
  margin-bottom: 0.75rem;
}
.bulk-field {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
  font-size: var(--fs-small);
}
.bulk-field > span {
  opacity: 0.8;
}
.bulk-combo {
  display: flex;
  gap: 0.3rem;
  flex-wrap: wrap;
}
.bulk-combo input {
  flex: 1;
  min-width: 8ch;
}
.bulk-optional > span {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
}
.bulk-replace {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.5rem 0.75rem 0.75rem;
  margin-bottom: 0.75rem;
}
.bulk-replace legend {
  font-size: var(--fs-small);
  opacity: 0.75;
  padding: 0 0.3rem;
}
.bulk-regex {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  font-size: var(--fs-small);
  white-space: nowrap;
}
</style>
