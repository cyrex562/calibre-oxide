<script setup lang="ts">
defineProps<{
  libraryPath: string;
}>();
</script>

<template>
  <section class="library-view">
    <p v-if="!libraryPath" class="empty">
      Pick a Calibre library above to see its contents.
    </p>
    <p v-else>
      Would show books from <code>{{ libraryPath }}</code> once the
      db-backed reader is wired through Tauri.
    </p>
  </section>
</template>

<style scoped>
.library-view {
  border: 1px solid #ddd;
  border-radius: 4px;
  padding: 1rem;
  min-height: 300px;
}
.empty {
  color: #888;
  font-style: italic;
}
code {
  background: #f0f0f0;
  padding: 0.1em 0.3em;
  border-radius: 3px;
}
</style>
