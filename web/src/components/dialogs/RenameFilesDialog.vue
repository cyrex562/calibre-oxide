<script setup lang="ts">
// Rename files (#885). The title of a book and the name of the file holding it
// are separate things; this changes only the second.
//
// Preview is mandatory rather than optional: a rename cannot be undone, so Apply
// stays disabled until the names on screen were computed for exactly this
// template and this selection.

import { computed, onMounted, ref, useId } from "vue";

import { renameFiles } from "../../library/api";
import { conventionFileTemplate, DEFAULT_RENAME_TEMPLATE, describeRename, initialRenameTemplate, previewIsCurrent, type RenamePreviewKey, summarizeRename } from "../../library/renameFiles";
import type { RenameFileResult } from "../../library/types";
import Dialog from "../Dialog.vue";

const props = defineProps<{ ids: number[] }>();
const emit = defineEmits<{
  close: [];
  /** Files were renamed on disk, so the `fmt_*` paths of the rows are stale. */
  renamed: [];
}>();
const formId = `rename-form-${useId()}`;

const renameTemplate = ref(DEFAULT_RENAME_TEMPLATE);
const renameBusy = ref(false);
const renameResults = ref<RenameFileResult[]>([]);
const renameApplied = ref(false);
const renameError = ref<string | null>(null);
const renamePreviewedFor = ref<RenamePreviewKey | null>(null);

const renameCanApply = computed(() => !renameBusy.value && !renameApplied.value && previewIsCurrent(renamePreviewedFor.value, renameTemplate.value, props.ids) && renameResults.value.some((r) => r.changed));
const renameSummary = computed(() => (renameResults.value.length ? describeRename(summarizeRename(renameResults.value), renameApplied.value) : ""));

onMounted(async () => {
  const ids = props.ids;
  if (ids.length === 0) return;
  // Prefilling with the book's current filename needs to know what it
  // is, and a dry run already reports that in `current` for every
  // book -- whatever template it was asked about. So the dialog opens
  // with one, purely to read the current names back; the proposal it
  // also returns is discarded, and no preview is shown until the user
  // asks for one.
  // Busy while the probe is in flight, so a fast typist's name cannot
  // be overwritten by the prefill arriving late.
  renameBusy.value = true;
  try {
    const { results } = await renameFiles(ids, "{title}", true);
    const initial = initialRenameTemplate(results.map((r) => r.current));
    // Several books (no single current name to start from): the library's convention.
    renameTemplate.value = initial === DEFAULT_RENAME_TEMPLATE ? ((await conventionFileTemplate()) ?? initial) : initial;
  } catch {
    renameTemplate.value = DEFAULT_RENAME_TEMPLATE;
  } finally {
    renameBusy.value = false;
  }
});

async function previewRename() {
  const ids = props.ids;
  if (ids.length === 0 || !renameTemplate.value.trim()) return;
  renameBusy.value = true;
  renameError.value = null;
  renameApplied.value = false;
  try {
    const { results } = await renameFiles(ids, renameTemplate.value, true);
    renameResults.value = results;
    renamePreviewedFor.value = { template: renameTemplate.value, bookIds: ids };
  } catch (e) {
    renameResults.value = [];
    renamePreviewedFor.value = null;
    renameError.value = e instanceof Error ? e.message : String(e);
  } finally {
    renameBusy.value = false;
  }
}

async function applyRename() {
  if (!renameCanApply.value) return;
  renameBusy.value = true;
  renameError.value = null;
  try {
    const { results } = await renameFiles(props.ids, renameTemplate.value, false);
    renameResults.value = results;
    renameApplied.value = true;
    // The `fmt_*` paths every row carries have moved.
    emit("renamed");
  } catch (e) {
    renameError.value = e instanceof Error ? e.message : String(e);
  } finally {
    renameBusy.value = false;
  }
}
</script>

<template>
    <Dialog :title="`Rename files (${ids.length})`" show-close size="lg" @close="emit('close')">
      <p class="news-hint">
        Renames the book files on disk. The title, author and library folder are not touched — this changes only what the file is called.
        Type a name, or a template such as <code>{{ '{title} - {authors}' }}</code>; the same fields save-to-disk accepts work here.
      </p>
      <form :id="formId" @submit.prevent="previewRename">
        <label class="news-field">
          New name or template
          <input v-model="renameTemplate" placeholder="{title} - {authors}" :disabled="renameBusy" />
        </label>
      </form>
      <p v-if="renameError" class="error">{{ renameError }}</p>
      <p v-if="renameSummary" class="news-hint">{{ renameSummary }}</p>
      <ul v-if="renameResults.length" class="manage-list">
        <li v-for="r in renameResults" :key="r.book_id">
          <span v-if="r.error" class="error">{{ r.title }}: {{ r.error }}</span>
          <span v-else-if="r.changed" class="manage-name">{{ r.current || "(unnamed)" }} → {{ r.proposed }}</span>
          <span v-else class="manage-name muted">{{ r.current }} (unchanged)</span>
        </li>
      </ul>
      <template #footer>
        <button type="submit" :form="formId" :disabled="renameBusy || !renameTemplate.trim()">{{ renameBusy ? "Working…" : "Preview" }}</button>
        <button type="button" :disabled="renameBusy" @click="emit('close')">Close</button>
        <!--
          Renaming cannot be undone, so this stays disabled until
          the table below was computed for exactly this template
          and this selection -- editing either invalidates it.
        -->
        <button type="button" class="primary" :disabled="!renameCanApply" @click="applyRename">Rename</button>
      </template>
    </Dialog>
</template>

<style scoped>
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.error {
  padding: 1em;
  color: var(--danger);
}
.news-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-size: var(--fs-small);
  color: var(--fg-muted);
  margin-bottom: 0.75em;
}
.news-field input,
.news-field textarea {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: 4px;
  resize: vertical;
}
.manage-name.muted {
  font-weight: 400;
  color: var(--fg-muted);
}
</style>
