<script setup lang="ts">
// Polish books (issue 1.10 of the #816 epic).
//
// `oeb::polish` is 92 files of ported, tested engine whose entry
// point had no caller anywhere -- no route, no CLI, no UI. This and
// POST /polish are the first.
//
// EPUB only, matching what the server can open. Books without an EPUB
// are reported per book rather than failing the batch.

import { ref } from "vue";

import { polishBooks, type PolishBookResult, type PolishOptions } from "../library/api";

const props = defineProps<{ bookIds: number[] }>();
const emit = defineEmits<{ close: []; done: [] }>();

/**
 * Grouped the way a user thinks about them rather than the order the
 * engine runs them in. `opf` is deliberately absent: `polish_one`
 * bails on it, so offering it would only produce an error.
 */
const ACTIONS: { key: keyof PolishOptions; label: string; hint?: string }[] = [
  { key: "smarten_punctuation", label: "Smarten punctuation", hint: "Straight quotes and hyphens become typographic" },
  { key: "upgrade_book", label: "Upgrade book internals", hint: "Convert to a newer EPUB structure" },
  { key: "remove_unused_css", label: "Remove unused CSS" },
  { key: "compress_images", label: "Compress images losslessly" },
  { key: "add_soft_hyphens", label: "Add soft hyphens" },
  { key: "remove_soft_hyphens", label: "Remove soft hyphens" },
  { key: "jacket", label: "Insert metadata jacket" },
  { key: "remove_jacket", label: "Remove metadata jacket" },
  { key: "embed", label: "Embed referenced fonts" },
  { key: "subset", label: "Subset embedded fonts" },
  { key: "download_external_resources", label: "Download external resources" },
];

/** Only meaningful alongside "Remove unused CSS". */
const CSS_CUSTOMIZATION: { key: keyof PolishOptions; label: string }[] = [
  { key: "remove_unused_classes", label: "Also remove unused classes" },
  { key: "merge_identical_selectors", label: "Merge identical selectors" },
  { key: "merge_rules_with_identical_properties", label: "Merge rules with identical properties" },
  { key: "remove_unreferenced_sheets", label: "Remove unreferenced stylesheets" },
  { key: "remove_ncx", label: "Remove the legacy NCX table of contents" },
];

const selected = ref<Partial<PolishOptions>>({});
const busy = ref(false);
const error = ref<string | null>(null);
const results = ref<PolishBookResult[] | null>(null);
const summary = ref<string | null>(null);

function toggle(key: keyof PolishOptions) {
  selected.value = { ...selected.value, [key]: !selected.value[key] };
}

// The customization flags change how "remove unused CSS" behaves; on
// their own they request no action at all, which the server refuses.
function anyActionSelected(): boolean {
  return ACTIONS.some((a) => selected.value[a.key]);
}

async function run() {
  if (!anyActionSelected()) {
    error.value = "Choose at least one action.";
    return;
  }
  busy.value = true;
  error.value = null;
  results.value = null;
  summary.value = null;
  try {
    const result = await polishBooks(props.bookIds, selected.value);
    results.value = result.books;
    summary.value = `${result.changed} book(s) changed${result.failed ? `, ${result.failed} failed` : ""}${result.books.length - result.changed - result.failed ? `, ${result.books.length - result.changed - result.failed} already clean` : ""}.`;
    emit("done");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="manage-backdrop" @click.self="emit('close')">
    <div class="manage-panel polish-panel">
      <h3>Polish {{ bookIds.length }} book(s)</h3>
      <p class="hint">Rewrites the EPUB in place. Books without an EPUB format are skipped and reported.</p>

      <ul class="polish-actions">
        <li v-for="a in ACTIONS" :key="a.key">
          <label class="field checkbox">
            <input type="checkbox" :checked="!!selected[a.key]" :disabled="busy" @change="toggle(a.key)" />
            {{ a.label }}
            <span v-if="a.hint" class="hint inline">{{ a.hint }}</span>
          </label>
        </li>
      </ul>

      <fieldset v-if="selected.remove_unused_css" class="polish-css">
        <legend>CSS options</legend>
        <label v-for="c in CSS_CUSTOMIZATION" :key="c.key" class="field checkbox">
          <input type="checkbox" :checked="!!selected[c.key]" :disabled="busy" @change="toggle(c.key)" />
          {{ c.label }}
        </label>
      </fieldset>

      <div class="polish-buttons">
        <button type="button" :disabled="busy" @click="run">{{ busy ? "Polishing…" : "Polish" }}</button>
        <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      </div>

      <p v-if="error" class="error">{{ error }}</p>
      <p v-if="summary" class="status">{{ summary }}</p>

      <ul v-if="results" class="polish-results">
        <li v-for="r in results" :key="r.book_id" :class="{ failed: !!r.error }">
          <strong>Book {{ r.book_id }}</strong>
          <span v-if="r.error" class="error">{{ r.error }}</span>
          <span v-else-if="!r.changed" class="hint">nothing to change</span>
          <span v-else class="polish-report">{{ (r.report ?? []).join(" · ") }}</span>
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.polish-panel {
  min-width: min(640px, 92vw);
}
.polish-actions {
  list-style: none;
  margin: 0 0 0.5rem;
  padding: 0;
  columns: 2;
  column-gap: 1.5rem;
}
.polish-actions li {
  break-inside: avoid;
  margin-bottom: 0.2rem;
}
.hint.inline {
  display: block;
  font-size: var(--fs-small);
  opacity: 0.65;
  margin-left: 1.5rem;
}
.polish-css {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.4rem 0.7rem 0.6rem;
  margin-bottom: 0.6rem;
}
.polish-css legend {
  font-size: var(--fs-small);
  opacity: 0.75;
}
.polish-buttons {
  display: flex;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
}
.polish-results {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 34vh;
  overflow-y: auto;
  font-size: var(--fs-small);
}
.polish-results li {
  display: flex;
  gap: 0.6rem;
  padding: 0.2rem 0;
  border-bottom: 1px solid var(--border);
}
.polish-report {
  opacity: 0.75;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
