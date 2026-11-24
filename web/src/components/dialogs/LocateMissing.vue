<script setup lang="ts">
// "Find it": looks for a missing file by the checksum recorded for it, among
// the library's untracked files, and offers the match to "Use this file".
// The Check library dialog hosts it in a missing book's row and does the
// relocating itself (`use`), so the verdict shows in its own message line.

import { ref } from "vue";

import { locateOrphan, type LocateResult } from "../../library/api";

const props = defineProps<{ bookId: number; format: string; disabled?: boolean }>();
const emit = defineEmits<{ use: [path: string] }>();

const searching = ref(false);
const result = ref<LocateResult | null>(null);
const error = ref<string | null>(null);

async function find() {
  searching.value = true;
  error.value = null;
  result.value = null;
  try {
    result.value = await locateOrphan(props.bookId, props.format);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    searching.value = false;
  }
}
</script>

<template>
  <span class="locate-missing" data-testid="locate-missing">
    <button type="button" :disabled="disabled || searching" @click="find">{{ searching ? "Looking…" : "Find it" }}</button>
    <span v-if="error" class="locate-note locate-error" role="alert">{{ error }}</span>
    <template v-else-if="result">
      <span v-if="!result.has_checksum" class="locate-note" data-testid="locate-no-checksum">No checksum was recorded for this file, so it cannot be found by content.</span>
      <span v-else-if="result.candidates.length === 0" class="locate-note" data-testid="locate-none">No file with the same content was found ({{ result.checked }} checked).</span>
      <span v-for="candidate in result.candidates" v-else :key="candidate.path" class="locate-candidate">
        <code>{{ candidate.path }}</code>
        <button type="button" :disabled="disabled" @click="emit('use', candidate.path)">Use this file</button>
      </span>
    </template>
  </span>
</template>

<style scoped>
.locate-missing {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.35em;
}
.locate-candidate {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
}
.locate-note {
  color: var(--fg-muted);
}
.locate-error {
  color: var(--danger);
}
</style>
