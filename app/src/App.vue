<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

// The real library UI is the browser app served from web/dist, spawned
// and navigated to by the Rust side (app/src-tauri/src/lib.rs) once a
// library is chosen or a previously-persisted one reopens. This view
// only ever shows during that brief window, or if no library has been
// chosen yet.

const status = ref<"checking" | "no-library" | "opening" | "error">(
  "checking",
);
const errorMessage = ref("");

async function checkPersistedLibrary() {
  const path = await invoke<string | null>("get_persisted_library");
  status.value = path ? "opening" : "no-library";
}

async function pickLibrary() {
  status.value = "opening";
  errorMessage.value = "";
  try {
    const opened = await invoke<boolean>("choose_library");
    if (!opened) {
      status.value = "no-library";
    }
    // On success the Rust side navigates the window itself; nothing
    // left to do here.
  } catch (e) {
    status.value = "error";
    errorMessage.value = String(e);
  }
}

onMounted(checkPersistedLibrary);
</script>

<template>
  <main class="splash">
    <h1>calibre-oxide</h1>

    <p v-if="status === 'checking'" class="hint">Checking for a library…</p>

    <p v-else-if="status === 'opening'" class="hint">
      Starting your library…
    </p>

    <div v-else-if="status === 'no-library'" class="picker">
      <p class="hint">Choose a Calibre library folder to get started.</p>
      <button @click="pickLibrary">Choose Library…</button>
    </div>

    <div v-else class="picker">
      <p class="error">Could not open that library: {{ errorMessage }}</p>
      <button @click="pickLibrary">Try Again…</button>
    </div>
  </main>
</template>

<style scoped>
.splash {
  height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1rem;
  font-family: system-ui, -apple-system, sans-serif;
}
.hint {
  color: #888;
}
.error {
  color: #b3261e;
}
.picker {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.75rem;
}
button {
  padding: 0.5rem 1.25rem;
  font-size: 1rem;
}
</style>
