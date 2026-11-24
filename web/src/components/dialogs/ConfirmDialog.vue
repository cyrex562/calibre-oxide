<script setup lang="ts">
// The body of `confirmDialog()` / `promptDialog()` / `alertDialog()` (see
// ./confirm.ts): a message, an optional text field, and Cancel / OK.

import { ref } from "vue";

import Dialog from "../Dialog.vue";

const props = defineProps<{
  message: string;
  title?: string;
  confirmLabel: string;
  cancelLabel: string | null;
  danger: boolean;
  /** Present for a prompt: the field's starting text. */
  input?: string;
}>();
const emit = defineEmits<{ done: [result: boolean | string | null] }>();

const text = ref(props.input ?? "");
const isPrompt = props.input !== undefined;

function accept() {
  emit("done", isPrompt ? text.value : true);
}
function cancel() {
  emit("done", isPrompt ? null : false);
}
</script>

<template>
  <Dialog :title="title" label="Confirm" size="sm" data-testid="confirm-dialog" @close="cancel">
    <form class="confirm-body" @submit.prevent="accept">
      <p class="confirm-message">{{ message }}</p>
      <input v-if="isPrompt" v-model="text" type="text" autofocus data-testid="confirm-input" aria-label="Value" />
    </form>
    <template #footer>
      <button v-if="cancelLabel !== null" type="button" data-testid="confirm-cancel" :autofocus="!isPrompt && danger" @click="cancel">{{ cancelLabel }}</button>
      <button type="button" :class="danger ? 'danger' : 'primary'" data-testid="confirm-ok" :autofocus="!isPrompt && !danger" @click="accept">{{ confirmLabel }}</button>
    </template>
  </Dialog>
</template>

<style scoped>
.confirm-body {
  display: flex;
  flex-direction: column;
  gap: var(--sp-5);
}
.confirm-message {
  margin: 0;
  white-space: pre-line;
}
</style>
