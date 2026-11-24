<script setup lang="ts">
// The library view is kept alive while the reader or settings are open, so
// coming back finds the query, scroll position, selection and details panel
// where they were (it reloads its list if something changed meanwhile: see
// library/libraryEpoch.ts). The reader and settings are not cached.
</script>

<template>
  <router-view v-slot="{ Component }">
    <keep-alive include="LibraryView">
      <component :is="Component" />
    </keep-alive>
  </router-view>
</template>

<style>
html,
body,
#app {
  height: 100%;
  margin: 0;
}
</style>
