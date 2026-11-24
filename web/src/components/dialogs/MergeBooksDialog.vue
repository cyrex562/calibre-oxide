<script setup lang="ts">
// Merge books: from a duplicate group ("Merge into…") or from a selection of two
// or more books (the "merge-books" action). The engine is
// crates/lexmechanic_db/src/merge.rs; this is the review step. Nothing changes
// until the user has seen a dry run and confirms it.

import { computed, ref, watch } from "vue";

import { mergeBooks } from "../../library/api";
import type { DuplicateBook, MergeMode, MergeResponse } from "../../library/api";
import { DEFAULT_DESTINATION_HINT, describeMergeResult, describeRecord, MERGE_MODE_LABELS, MERGE_MODE_NOTES, mergeOutcome, summarizeMerge, type MergeOutcome } from "../../library/merge";
import Dialog from "../Dialog.vue";

const props = defineProps<{
  books: DuplicateBook[];
  /** The record to keep, to start with. */
  dest: number | null;
}>();
const emit = defineEmits<{
  close: [];
  /** The merge ran; `description` says what it did. */
  merged: [description: string, outcome: MergeOutcome];
}>();

const mergeDest = ref<number | null>(props.dest);
const mergeMode = ref<MergeMode>("merge_and_delete");
const mergeKeepDuplicates = ref(false);
const mergePreview = ref<MergeResponse | null>(null);
const mergeBusy = ref(false);
const mergeError = ref<string | null>(null);
const mergeModes = Object.keys(MERGE_MODE_LABELS) as MergeMode[];
const mergePreviewLines = computed(() => (mergePreview.value ? summarizeMerge(mergePreview.value) : []));

// Any change to what will be merged invalidates the preview the user saw.
watch([mergeDest, mergeMode, mergeKeepDuplicates], () => {
  mergePreview.value = null;
});

async function reviewMerge() {
  if (mergeDest.value === null) return;
  mergeBusy.value = true;
  mergeError.value = null;
  try {
    mergePreview.value = await mergeBooks({ dest: mergeDest.value, sources: props.books.map((b) => b.book_id).filter((id) => id !== mergeDest.value), mode: mergeMode.value, keep_duplicate_formats: mergeKeepDuplicates.value, dry_run: true });
  } catch (e) {
    mergeError.value = e instanceof Error ? e.message : String(e);
  } finally {
    mergeBusy.value = false;
  }
}

async function confirmMerge() {
  if (mergeDest.value === null || !mergePreview.value) return;
  mergeBusy.value = true;
  mergeError.value = null;
  try {
    const result = await mergeBooks({ dest: mergeDest.value, sources: props.books.map((b) => b.book_id).filter((id) => id !== mergeDest.value), mode: mergeMode.value, keep_duplicate_formats: mergeKeepDuplicates.value });
    mergePreview.value = null;
    emit("merged", describeMergeResult(result), mergeOutcome(result));
  } catch (e) {
    mergeError.value = e instanceof Error ? e.message : String(e);
  } finally {
    mergeBusy.value = false;
  }
}

</script>

<template>
    <Dialog :title="`Merge books (${books.length})`" show-close size="lg" @close="emit('close')">
      <p class="news-hint">
        Choose the record to keep. The others' formats and any metadata the destination lacks are merged into it.
      </p>
      <details class="news-hint">
        <summary>Why is one pre-selected?</summary>
        {{ DEFAULT_DESTINATION_HINT }}
      </details>
      <ul class="manage-list">
        <li v-for="b in books" :key="b.book_id">
          <label>
            <input v-model="mergeDest" type="radio" name="merge-dest" :value="b.book_id" :disabled="mergeBusy" />
            {{ b.title }} — {{ b.authors.join(", ") }}
            <small v-if="describeRecord(b)" class="news-hint">({{ describeRecord(b) }})</small>
          </label>
        </li>
      </ul>
      <label>
        Mode
        <select v-model="mergeMode" :disabled="mergeBusy">
          <option v-for="m in mergeModes" :key="m" :value="m">{{ MERGE_MODE_LABELS[m] }}</option>
        </select>
      </label>
      <p class="news-hint">{{ MERGE_MODE_NOTES[mergeMode] }}</p>
      <label v-if="mergeMode !== 'merge_keep_sources'">
        <input v-model="mergeKeepDuplicates" type="checkbox" :disabled="mergeBusy" />
        Keep a differing copy of a format the destination already has, as a data file
      </label>
      <p v-if="mergeError" class="error">{{ mergeError }}</p>
      <div v-if="mergePreview" role="group" aria-label="What will happen">
        <h4>This will:</h4>
        <ul>
          <li v-for="(line, i) in mergePreviewLines" :key="i">{{ line }}</li>
        </ul>
      </div>
      <template #footer>
        <button type="button" @click="emit('close')">Cancel</button>
        <button v-if="!mergePreview" type="button" class="primary" :disabled="mergeBusy || mergeDest === null" @click="reviewMerge">Review…</button>
        <button v-else type="button" class="primary" :disabled="mergeBusy || !mergePreview.ok" @click="confirmMerge">{{ mergeBusy ? "Merging…" : "Merge" }}</button>
      </template>
    </Dialog>
</template>

<style scoped>
.error {
  padding: 1em;
  color: var(--danger);
}
</style>
