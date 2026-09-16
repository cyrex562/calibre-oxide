<script setup lang="ts">
import { onMounted, ref } from "vue";
import { isTauri, tauriInvoke } from "../tauri";
import { DEFAULT_LIBRARY_PREFS, DEFAULT_READER_PREFS, fetchProfile, LIBRARY_PREFS_PROFILE, READER_PREFS_PROFILE, saveProfile, type LibraryPrefs, type ReaderPrefs } from "./api";

const libraryPrefs = ref<LibraryPrefs>({ ...DEFAULT_LIBRARY_PREFS });
const readerPrefs = ref<ReaderPrefs>({ ...DEFAULT_READER_PREFS });
const loading = ref(true);
const savedMessage = ref<string | null>(null);
const error = ref<string | null>(null);

const autoReopen = ref(true);
const showAppSettings = ref(false);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const [lib, reader] = await Promise.all([fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE), fetchProfile<ReaderPrefs>(READER_PREFS_PROFILE)]);
    if (lib) libraryPrefs.value = { ...DEFAULT_LIBRARY_PREFS, ...lib };
    if (reader) readerPrefs.value = { ...DEFAULT_READER_PREFS, ...reader };
    if (isTauri()) {
      showAppSettings.value = true;
      autoReopen.value = await tauriInvoke<boolean>("get_auto_reopen");
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function saveLibraryPrefs() {
  savedMessage.value = null;
  error.value = null;
  try {
    await saveProfile(LIBRARY_PREFS_PROFILE, libraryPrefs.value as unknown as Record<string, unknown>);
    savedMessage.value = "Library preferences saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function saveReaderPrefs() {
  savedMessage.value = null;
  error.value = null;
  try {
    await saveProfile(READER_PREFS_PROFILE, readerPrefs.value as unknown as Record<string, unknown>);
    savedMessage.value = "Reading preferences saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function toggleAutoReopen() {
  error.value = null;
  try {
    await tauriInvoke<void>("set_auto_reopen", { enabled: autoReopen.value });
    savedMessage.value = "App settings saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div class="settings">
    <header class="toolbar">
      <router-link to="/" class="back">Library</router-link>
      <h2>Settings</h2>
    </header>

    <p v-if="loading" class="status">Loading…</p>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="savedMessage" class="saved">{{ savedMessage }}</p>

    <template v-if="!loading">
      <section class="pane">
        <h3>Library</h3>
        <label class="field">
          Default sort
          <select v-model="libraryPrefs.sort">
            <option value="timestamp">Date added</option>
            <option value="title">Title</option>
            <option value="authors">Author</option>
            <option value="pubdate">Publication date</option>
            <option value="rating">Rating</option>
            <option value="series">Series</option>
          </select>
        </label>
        <label class="field">
          Default sort order
          <select v-model="libraryPrefs.sortOrder">
            <option value="asc">Ascending</option>
            <option value="desc">Descending</option>
          </select>
        </label>
        <label class="field">
          Books per page
          <input v-model.number="libraryPrefs.pageSize" type="number" min="6" max="200" step="1" />
        </label>
        <label class="field">
          When adding a book that looks like a duplicate
          <select v-model="libraryPrefs.duplicateDefault">
            <option value="ask">Ask every time</option>
            <option value="add">Always add it anyway</option>
            <option value="skip">Always skip it</option>
          </select>
        </label>
        <button type="button" @click="saveLibraryPrefs">Save library settings</button>
      </section>

      <section class="pane">
        <h3>Reading</h3>
        <label class="field">
          Font size ({{ readerPrefs.fontSizePercent }}%)
          <input v-model.number="readerPrefs.fontSizePercent" type="range" min="60" max="220" step="10" />
        </label>
        <label class="field">
          Theme
          <select v-model="readerPrefs.theme">
            <option value="light">Light</option>
            <option value="dark">Dark</option>
            <option value="sepia">Sepia</option>
          </select>
        </label>
        <button type="button" @click="saveReaderPrefs">Save reading settings</button>
      </section>

      <section v-if="showAppSettings" class="pane">
        <h3>App</h3>
        <label class="field checkbox">
          <input v-model="autoReopen" type="checkbox" @change="toggleAutoReopen" />
          Reopen the last library automatically on launch
        </label>
      </section>
    </template>
  </div>
</template>

<style scoped>
.settings {
  max-width: 640px;
  margin: 0 auto;
  padding: 1em;
  display: flex;
  flex-direction: column;
  gap: 1.25em;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 1em;
}
.toolbar h2 {
  margin: 0;
}
.pane {
  display: flex;
  flex-direction: column;
  gap: 0.75em;
  padding: 1em;
  border: 1px solid #ddd;
  border-radius: 6px;
}
.pane h3 {
  margin: 0;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
}
.field.checkbox {
  flex-direction: row;
  align-items: center;
}
.error {
  color: #b00020;
}
.saved {
  color: #1b7f3a;
}
</style>
