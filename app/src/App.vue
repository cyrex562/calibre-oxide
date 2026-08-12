<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LibraryView from "./components/LibraryView.vue";

const libraryPath = ref("");
const backendGreeting = ref("");

async function pingBackend() {
  backendGreeting.value = await invoke<string>("ping");
}
</script>

<template>
  <main class="app-shell">
    <header>
      <h1>calibre-oxide</h1>
      <p class="tagline">A fault-tolerant organizer for your media.</p>
    </header>

    <section class="library-picker">
      <input
        v-model="libraryPath"
        placeholder="Path to a Calibre library (metadata.db)"
      />
      <button @click="pingBackend">Ping backend</button>
      <p v-if="backendGreeting" class="ok">{{ backendGreeting }}</p>
    </section>

    <LibraryView :library-path="libraryPath" />
  </main>
</template>

<style scoped>
.app-shell {
  padding: 1rem 2rem;
  font-family: system-ui, -apple-system, sans-serif;
}
header h1 {
  margin-bottom: 0.25rem;
}
.tagline {
  color: #888;
  margin-top: 0;
}
.library-picker {
  margin: 1rem 0;
  display: flex;
  gap: 0.5rem;
  align-items: center;
}
.library-picker input {
  flex: 1;
  padding: 0.5rem;
}
.ok {
  color: #2a7f2a;
  margin: 0;
}
</style>
