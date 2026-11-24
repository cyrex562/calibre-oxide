<script setup lang="ts">
// Save to disk (#751) -- real POST /save-to-disk against the books the view
// acts on, evaluating a real calibre-template-language `{field}`-shorthand path
// template per book server-side.

import { onMounted, ref, useId } from "vue";

import { saveToDisk } from "../../library/api";
import type { SaveToDiskResult } from "../../library/api";
import { DEFAULT_LIBRARY_PREFS, DEFAULT_SAVE_TO_DISK_TEMPLATE, fetchProfile, LIBRARY_PREFS_PROFILE, saveProfile, type LibraryPrefs } from "../../settings/api";
import Dialog from "../Dialog.vue";

const props = defineProps<{ ids: number[] }>();
const emit = defineEmits<{ close: [] }>();
const formId = `save-to-disk-form-${useId()}`;

const saveToDiskTemplate = ref(DEFAULT_SAVE_TO_DISK_TEMPLATE);
const saveToDiskDest = ref("");
const saveToDiskBusy = ref(false);
const saveToDiskResults = ref<SaveToDiskResult[]>([]);
const saveToDiskError = ref<string | null>(null);

// Restored on open rather than in `loadLibraryPrefs`, which runs before this exists.
onMounted(() => void restoreSaveTemplate());

/**
 * Brings back the user's own path template (#4.4).
 *
 * The template was always editable, but reset to the default on every
 * open -- so anyone with their own folder layout retyped it each
 * time, which is exactly the kind of small tax that makes a feature
 * feel unfinished.
 */
async function restoreSaveTemplate() {
  try {
    const prefs = await fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE);
    if (prefs?.saveToDiskTemplate) saveToDiskTemplate.value = prefs.saveToDiskTemplate;
  } catch {
    // Keep the default; this is a convenience, not a requirement.
  }
}

async function persistSaveTemplate() {
  try {
    const prefs = (await fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE)) ?? DEFAULT_LIBRARY_PREFS;
    await saveProfile(LIBRARY_PREFS_PROFILE, { ...prefs, saveToDiskTemplate: saveToDiskTemplate.value });
  } catch (e) {
    console.error("failed to remember the save-to-disk template", e);
  }
}

async function runSaveToDisk() {
  const ids = props.ids;
  if (ids.length === 0 || !saveToDiskDest.value.trim()) return;
  saveToDiskBusy.value = true;
  saveToDiskError.value = null;
  saveToDiskResults.value = [];
  try {
    const { results } = await saveToDisk(ids, saveToDiskTemplate.value, saveToDiskDest.value.trim());
    saveToDiskResults.value = results;
    // Remembered only after a save that worked, so a template being
    // typed and abandoned does not become the stored one.
    void persistSaveTemplate();
  } catch (e) {
    saveToDiskError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saveToDiskBusy.value = false;
  }
}
</script>

<template>
    <Dialog title="Save to disk" show-close size="lg" @close="emit('close')">
      <p class="news-hint">Exports the selected book(s) into a folder tree named from a real calibre template. Available fields include {title}, {authors}, {author_sort}, {series}, and any custom column (e.g. {#shelf}).</p>
      <form :id="formId" @submit.prevent="runSaveToDisk">
        <label class="news-field">
          Path template
          <input v-model="saveToDiskTemplate" placeholder="{author_sort}/{title}/{title} - {authors}" :disabled="saveToDiskBusy" />
        </label>
        <label class="news-field">
          Destination folder (absolute path)
          <input v-model="saveToDiskDest" placeholder="/home/me/Books" :disabled="saveToDiskBusy" />
        </label>
      </form>
      <p v-if="saveToDiskError" class="error">{{ saveToDiskError }}</p>
      <ul v-if="saveToDiskResults.length" class="manage-list">
        <li v-for="r in saveToDiskResults" :key="r.book_id">
          <span v-if="r.ok" class="manage-name">Book {{ r.book_id }}: saved {{ r.paths?.length }} file(s)</span>
          <span v-else class="error">Book {{ r.book_id }}: {{ r.error }}</span>
        </li>
      </ul>
      <template #footer>
        <button type="button" :disabled="saveToDiskBusy" @click="emit('close')">Close</button>
        <button type="submit" :form="formId" class="primary" :disabled="saveToDiskBusy || !saveToDiskDest.trim()">{{ saveToDiskBusy ? "Saving…" : "Save" }}</button>
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
  border-radius: var(--radius-md);
  resize: vertical;
}
</style>
