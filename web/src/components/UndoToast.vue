<script setup lang="ts">
// "Moved 3 books to Trash · Undo" (see ../library/deleteFlow.ts).

import { dismissToast, runToastAction, toastState } from "../library/deleteFlow";
</script>

<template>
  <div v-if="toastState.toast" class="toast" role="status" aria-live="polite" data-testid="undo-toast">
    <span class="msg">{{ toastState.toast.result ?? toastState.toast.message }}</span>
    <button v-if="toastState.toast.actionLabel" type="button" class="link" :disabled="toastState.toast.busy" data-testid="undo-toast-action" @click="runToastAction">
      {{ toastState.toast.busy ? "Restoring…" : toastState.toast.actionLabel }}
    </button>
    <button type="button" class="link close" aria-label="Dismiss" @click="dismissToast">✕</button>
  </div>
</template>

<style scoped>
.toast {
  position: fixed;
  left: 50%;
  bottom: var(--sp-7);
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: var(--sp-5);
  padding: var(--sp-4) var(--sp-6);
  background: var(--bg-raised);
  color: var(--fg);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-menu);
  z-index: 30;
}
.link {
  border: none;
  background: none;
  height: auto;
  padding: 0 var(--sp-2);
  color: var(--accent);
  font-weight: 600;
}
.link.close {
  color: var(--fg-muted);
  font-weight: normal;
}
</style>
