<script setup lang="ts">
// Copy (or move) the selected books into another library this server hosts (#959).

import { ref, useId } from "vue";

import { copyToLibrary, describeCopyOutcome } from "../../library/api";
import Dialog from "../Dialog.vue";

const props = defineProps<{
  ids: number[];
  /** The other libraries this server hosts. */
  targets: { id: string; name: string }[];
}>();
const emit = defineEmits<{
  close: [];
  /** Books left this library (a move): the list is stale. */
  moved: [];
}>();
const formId = `copy-form-${useId()}`;

const copyBusy = ref(false);
const copyTarget = ref(props.targets[0]?.id ?? "");
const copyMove = ref(false);
const copyResults = ref<string[]>([]);
const copyDone = ref(false);
const copyError = ref<string | null>(null);

async function runCopyToLibrary() {
  const ids = props.ids;
  if (ids.length === 0 || !copyTarget.value) return;
  copyBusy.value = true;
  copyError.value = null;
  try {
    const response = await copyToLibrary(copyTarget.value, ids, copyMove.value);
    // Per book, because one already present in the target is skipped
    // while its neighbours are copied.
    copyResults.value = Object.entries(response).map(([bookId, outcome]) => describeCopyOutcome(bookId, outcome));
    copyDone.value = true;
    // A move takes books out of this library, so the grid is stale.
    if (copyMove.value) emit("moved");
  } catch (e) {
    copyError.value = e instanceof Error ? e.message : String(e);
  } finally {
    copyBusy.value = false;
  }
}
</script>

<template>
    <Dialog title="Copy to library" show-close size="md" @close="emit('close')">
      <p v-if="targets.length === 0" class="news-hint">
        This server is hosting only this library, so there is nowhere to copy to. Open another library once and it becomes
        available here.
      </p>
      <template v-else>
        <p class="news-hint">Copies the selected book(s), with their formats and cover, into another library this server hosts.</p>
        <form :id="formId" @submit.prevent="runCopyToLibrary">
          <label class="news-field">
            Destination library
            <select v-model="copyTarget" :disabled="copyBusy">
              <option v-for="target in targets" :key="target.id" :value="target.id">{{ target.name }}</option>
            </select>
          </label>
          <label class="news-field">
            <input v-model="copyMove" type="checkbox" :disabled="copyBusy" />
            Remove from this library afterwards (move rather than copy)
          </label>
        </form>
      </template>
      <p v-if="copyError" class="error">{{ copyError }}</p>
      <ul v-if="copyDone && copyResults.length" class="manage-list">
        <li v-for="(line, i) in copyResults" :key="i"><span class="manage-name">{{ line }}</span></li>
      </ul>
      <template #footer>
        <button type="button" :disabled="copyBusy" @click="emit('close')">Close</button>
        <button v-if="targets.length > 0" type="submit" :form="formId" class="primary" :disabled="copyBusy || !copyTarget">
          {{ copyBusy ? "Copying…" : `${copyMove ? "Move" : "Copy"} ${ids.length} book(s)` }}
        </button>
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
</style>
