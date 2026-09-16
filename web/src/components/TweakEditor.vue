<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { commitTweakSession, discardTweakSession, fetchTweakFile, openTweakSession, saveTweakFile } from "../library/tweak";

const props = defineProps<{ bookId: number }>();
const emit = defineEmits<{ close: []; updated: [] }>();

const loading = ref(true);
const error = ref<string | null>(null);
const sessionId = ref<string | null>(null);
const files = ref<string[]>([]);
const selectedFile = ref<string | null>(null);
const content = ref("");
const savedContent = ref(""); // last-saved-or-loaded content, to detect unsaved edits
const fileError = ref<string | null>(null);
const saving = ref(false);
const committing = ref(false);

const dirty = () => content.value !== savedContent.value;

async function open() {
  loading.value = true;
  error.value = null;
  try {
    const session = await openTweakSession(props.bookId);
    sessionId.value = session.session_id;
    files.value = session.files;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
void open();

async function selectFile(name: string) {
  if (!sessionId.value) return;
  if (dirty() && !confirm(`Discard unsaved changes to "${selectedFile.value}"?`)) return;
  fileError.value = null;
  try {
    const text = await fetchTweakFile(sessionId.value, name);
    selectedFile.value = name;
    content.value = text;
    savedContent.value = text;
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  }
}

async function save() {
  if (!sessionId.value || !selectedFile.value) return;
  saving.value = true;
  fileError.value = null;
  try {
    await saveTweakFile(sessionId.value, selectedFile.value, content.value);
    savedContent.value = content.value;
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

async function commitAndClose() {
  if (!sessionId.value) return;
  if (dirty() && !confirm("You have unsaved changes to the current file. Save them before committing?")) return;
  committing.value = true;
  fileError.value = null;
  try {
    if (dirty()) await save();
    await commitTweakSession(sessionId.value);
    sessionId.value = null;
    emit("updated");
    emit("close");
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  } finally {
    committing.value = false;
  }
}

function discardAndClose() {
  if (sessionId.value) void discardTweakSession(sessionId.value);
  sessionId.value = null;
  emit("close");
}

// A real cleanup best-effort, not a guarantee: this port has no
// idle-session reaper (see tweak.rs's own doc) -- closing via the
// browser tab/reload rather than this component's own buttons still
// leaks a session server-side, same disclosed narrowing as
// render_jobs/conversion_jobs.
onBeforeUnmount(() => {
  if (sessionId.value) void discardTweakSession(sessionId.value);
});
</script>

<template>
  <div class="backdrop">
    <div class="panel">
      <button class="close" @click="discardAndClose">✕</button>
      <h3>Tweak Book</h3>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else>
        <p class="hint">Plain-text editing of this EPUB's own internal files. No rich editor or live preview yet -- open the edited book in the reader afterward to check your changes.</p>
        <div class="editor">
          <ul class="file-list">
            <li v-for="name in files" :key="name">
              <button type="button" :class="{ active: name === selectedFile }" @click="selectFile(name)">{{ name }}</button>
            </li>
          </ul>
          <div class="file-content">
            <p v-if="!selectedFile" class="hint">Select a file to edit.</p>
            <textarea v-else v-model="content" spellcheck="false"></textarea>
          </div>
        </div>
        <p v-if="fileError" class="error">{{ fileError }}</p>
        <div class="actions">
          <button type="button" :disabled="!selectedFile || saving" @click="save">{{ saving ? "Saving…" : "Save file" }}</button>
          <button type="button" class="read" :disabled="committing" @click="commitAndClose">{{ committing ? "Saving book…" : "Save book" }}</button>
          <button type="button" :disabled="committing" @click="discardAndClose">Discard all changes</button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}
.panel {
  background: #fff;
  border-radius: 6px;
  padding: 1.5em;
  width: 90%;
  max-width: 900px;
  height: 80vh;
  overflow: hidden;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 0.75em;
}
.close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: 1.1em;
  cursor: pointer;
}
h3 {
  margin: 0;
}
.hint {
  color: #888;
  font-size: 0.85em;
  margin: 0;
}
.editor {
  display: flex;
  gap: 1em;
  flex: 1;
  min-height: 0;
}
.file-list {
  list-style: none;
  margin: 0;
  padding: 0;
  width: 260px;
  flex-shrink: 0;
  overflow: auto;
  border: 1px solid #ddd;
  border-radius: 4px;
}
.file-list button {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  padding: 0.35em 0.5em;
  cursor: pointer;
  font: inherit;
  font-size: 0.85em;
  word-break: break-all;
}
.file-list button.active {
  background: #2a6df4;
  color: #fff;
}
.file-content {
  flex: 1;
  min-width: 0;
  display: flex;
}
.file-content textarea {
  flex: 1;
  font-family: ui-monospace, monospace;
  font-size: 0.85em;
  padding: 0.5em;
  border: 1px solid #ddd;
  border-radius: 4px;
  resize: none;
}
.actions {
  display: flex;
  gap: 0.5em;
}
.read {
  background: #2a6df4;
  color: #fff;
  border: none;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.error {
  color: #b00020;
}
</style>
