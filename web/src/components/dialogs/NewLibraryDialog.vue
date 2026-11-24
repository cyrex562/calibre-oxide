<script setup lang="ts">
// "New library": a name, then the shell's folder picker. The caller creates
// the library; this holds the text and reports the submit.

import Dialog from "../Dialog.vue";

defineProps<{ busy: boolean; error: string | null }>();
const name = defineModel<string>("name", { required: true });
const emit = defineEmits<{ close: []; submit: [] }>();
</script>

<template>
  <Dialog title="New library" show-close :busy="busy" size="md" @close="emit('close')">
    <p class="hint">Name the library, then choose the folder to create it in. It opens straight away, empty and ready for books.</p>
    <form id="new-library-form" class="manage-form" @submit.prevent="emit('submit')">
      <input v-model="name" placeholder="Library name" :disabled="busy" autofocus />
    </form>
    <p v-if="error" class="error">{{ error }}</p>
    <template #footer>
      <button type="button" :disabled="busy" @click="emit('close')">Cancel</button>
      <button type="submit" form="new-library-form" class="primary" :disabled="busy || !name.trim()">
        {{ busy ? "Creating…" : "Choose folder…" }}
      </button>
    </template>
  </Dialog>
</template>
