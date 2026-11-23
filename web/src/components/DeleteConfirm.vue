<script setup lang="ts">
// The confirm every delete asks (see ../library/deleteFlow.ts): it names the
// titles instead of only counting them, says when some are hidden by the
// current search, and says the books go to the library Trash.

import { nextTick, ref, watch } from "vue";

import { answerTrashConfirm, confirmState } from "../library/deleteFlow";

const cancelButton = ref<HTMLButtonElement | null>(null);

// Cancel has focus, so a stray Enter cannot delete.
watch(
  () => confirmState.open,
  (open) => {
    if (open) void nextTick(() => cancelButton.value?.focus());
  },
);

function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.stopPropagation();
    answerTrashConfirm(false);
  }
}
</script>

<template>
  <div v-if="confirmState.open && confirmState.text" class="manage-backdrop" @click.self="answerTrashConfirm(false)" @keydown="onKey">
    <div class="manage-panel delete-confirm" role="dialog" aria-modal="true" aria-labelledby="delete-confirm-q" data-testid="delete-confirm">
      <h3 id="delete-confirm-q">{{ confirmState.text.question }}</h3>
      <ul class="titles" data-testid="delete-titles">
        <li v-for="(t, i) in confirmState.text.titles" :key="i">{{ t }}</li>
        <li v-if="confirmState.text.moreCount > 0" class="more">…and {{ confirmState.text.moreCount.toLocaleString("en-US") }} more</li>
      </ul>
      <p v-if="confirmState.text.hiddenNote" class="warn-note" data-testid="delete-hidden">{{ confirmState.text.hiddenNote }}</p>
      <p v-for="w in confirmState.warnings" :key="w" class="warn-note" data-testid="delete-warning">{{ w }}</p>
      <p class="hint">{{ confirmState.text.note }}</p>
      <div class="actions">
        <button ref="cancelButton" type="button" @click="answerTrashConfirm(false)">Cancel</button>
        <button type="button" class="danger" data-testid="delete-confirm-ok" @click="answerTrashConfirm(true)">Move to Trash</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.delete-confirm {
  max-width: 440px;
  gap: 0.75em;
}
.titles {
  margin: 0;
  padding-left: 1.2em;
  max-height: 14em;
  overflow: auto;
}
.titles .more {
  list-style: none;
  margin-left: -1.2em;
  color: var(--fg-muted);
}
.warn-note {
  margin: 0;
  color: var(--warning);
  font-weight: 600;
}
.actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--sp-4);
}
</style>
