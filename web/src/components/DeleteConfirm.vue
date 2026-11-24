<script setup lang="ts">
// The confirm every delete asks (see ../library/deleteFlow.ts): it names the
// titles instead of only counting them, says when some are hidden by the
// current search, and says the books go to the library Trash.

import { answerTrashConfirm, confirmState } from "../library/deleteFlow";
import Dialog from "./Dialog.vue";
</script>

<template>
  <!-- Cancel has autofocus, so a stray Enter cannot delete. -->
  <Dialog v-if="confirmState.open && confirmState.text" :title="confirmState.text.question" size="sm" class="delete-confirm" data-testid="delete-confirm" @close="answerTrashConfirm(false)">
    <ul class="titles" data-testid="delete-titles">
      <li v-for="(t, i) in confirmState.text.titles" :key="i">{{ t }}</li>
      <li v-if="confirmState.text.moreCount > 0" class="more">…and {{ confirmState.text.moreCount.toLocaleString("en-US") }} more</li>
    </ul>
    <p v-if="confirmState.text.hiddenNote" class="warn-note" data-testid="delete-hidden">{{ confirmState.text.hiddenNote }}</p>
    <p v-for="w in confirmState.warnings" :key="w" class="warn-note" data-testid="delete-warning">{{ w }}</p>
    <p class="hint">{{ confirmState.text.note }}</p>
    <template #footer>
      <button type="button" autofocus @click="answerTrashConfirm(false)">Cancel</button>
      <button type="button" class="danger" data-testid="delete-confirm-ok" @click="answerTrashConfirm(true)">Move to Trash</button>
    </template>
  </Dialog>
</template>

<style scoped>
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
</style>
