<script setup lang="ts">
// Files an import found to look like books already in the library: tick the
// ones to add anyway, the rest are skipped. Closing it (Escape) skips them
// all. The list and its ticks belong to the caller; this reports the choice.

import type { PendingDuplicate } from "../../library/importFiles";
import Dialog from "../Dialog.vue";

export type PendingDuplicateRow = PendingDuplicate & { checked: boolean };

const props = defineProps<{ duplicates: PendingDuplicateRow[]; busy: boolean }>();
const emit = defineEmits<{ resolve: [mode: "skip-all" | "add-all" | "checked"] }>();

const tickedCount = () => props.duplicates.filter((d) => d.checked).length;
</script>

<template>
  <Dialog
    :title="`${duplicates.length.toLocaleString('en-US')} ${duplicates.length === 1 ? 'file looks' : 'files look'} like books already in the library`"
    :busy="busy"
    :close-on-backdrop="false"
    size="md"
    data-testid="import-dupes"
    @close="emit('resolve', 'skip-all')"
  >
    <p class="hint">Tick the ones to add anyway. The rest are skipped.</p>
    <ul class="dupes-list">
      <li v-for="d in duplicates" :key="d.file.name" class="dupe">
        <label>
          <input v-model="d.checked" type="checkbox" :disabled="busy" />
          <span class="dupe-name">{{ d.file.name }}</span>
        </label>
        <span class="hint">already: {{ d.existing.join(", ") }}</span>
      </li>
    </ul>
    <template #footer>
      <button type="button" :disabled="busy" data-testid="dupes-skip-all" @click="emit('resolve', 'skip-all')">Skip all</button>
      <button type="button" :disabled="busy" data-testid="dupes-add-all" @click="emit('resolve', 'add-all')">Add all</button>
      <button type="button" class="primary" :disabled="busy || tickedCount() === 0" data-testid="dupes-add-checked" @click="emit('resolve', 'checked')">
        {{ busy ? "Adding…" : `Add ${tickedCount()} ticked` }}
      </button>
    </template>
  </Dialog>
</template>

<style scoped>
.dupes-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 40vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: var(--sp-3);
}
.dupe {
  display: flex;
  flex-direction: column;
}
.dupe-name {
  font-weight: 600;
}
</style>
