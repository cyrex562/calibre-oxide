<script setup lang="ts">
// "Switch library": the recently opened libraries, and a folder picker for
// another. The caller does the switching; this reports what was chosen.

import Dialog from "../Dialog.vue";

defineProps<{ recent: string[]; switching: boolean; error: string | null }>();
const emit = defineEmits<{ close: []; chooseRecent: [path: string]; browse: [] }>();
</script>

<template>
  <Dialog title="Switch library" show-close :busy="switching" size="md" @close="emit('close')">
    <ul v-if="recent.length" class="manage-list">
      <li v-for="path in recent" :key="path">
        <button type="button" :disabled="switching" @click="emit('chooseRecent', path)">{{ path }}</button>
      </li>
    </ul>
    <p v-else class="news-hint">No other recently-opened libraries yet.</p>
    <p v-if="error" class="error">{{ error }}</p>
    <template #footer>
      <button type="button" class="primary" :disabled="switching" @click="emit('browse')">Browse for another…</button>
      <button type="button" :disabled="switching" @click="emit('close')">Close</button>
    </template>
  </Dialog>
</template>
