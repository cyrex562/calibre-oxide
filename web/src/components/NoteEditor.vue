<script setup lang="ts">
import DOMPurify from "dompurify";
import { computed, nextTick, ref, watch } from "vue";
import { fileToDataUrl } from "../library/api";
import { fetchNoteByName, saveNote, type NoteImageSpec } from "../library/notes";

const props = defineProps<{ field: string; itemName: string }>();
const emit = defineEmits<{ close: [] }>();

const loading = ref(true);
const error = ref<string | null>(null);
const itemId = ref<number | null>(null);
const html = ref("");
const editing = ref(false);
const saving = ref(false);
const saveError = ref<string | null>(null);
const editorEl = ref<HTMLDivElement | null>(null);
const imageInput = ref<HTMLInputElement | null>(null);

async function load() {
  loading.value = true;
  error.value = null;
  editing.value = false;
  try {
    const note = await fetchNoteByName(props.field, props.itemName);
    itemId.value = note.item_id;
    html.value = note.html;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
watch(() => [props.field, props.itemName], load, { immediate: true });

// Real stored-XSS mitigation: `html` is rendered/injected as live DOM
// (`v-html` below, and a direct `.innerHTML =` set at edit-start), not
// escaped text. Notes are ordinarily self-authored through this same
// editor, but a library opened from elsewhere (a real calibre desktop
// install, a shared/imported metadata.db) could carry a note written
// by a completely different tool with no such restriction -- merely
// *viewing* an unsanitized note like that would execute arbitrary
// script same-origin with this app's own API access (delete books,
// exfiltrate data, ...). `notes.rs`'s own doc already flags and
// mitigates the analogous risk for embedded *image* resources
// (content-type allowlist); this is the same class of risk for the
// note body itself, sanitized client-side since this port doesn't run
// a server-side HTML sanitizer either.
const sanitizedHtml = computed(() => DOMPurify.sanitize(html.value || "<p><em>No note yet.</em></p>"));

async function startEditing() {
  saveError.value = null;
  editing.value = true;
  // The editable div isn't in the DOM until the v-if flips -- set its
  // starting content once it is. Not a v-model/v-html binding: a
  // contenteditable element fighting Vue's own reactive re-rendering
  // of v-html on every keystroke is a well-known breakage (cursor
  // jumps to the start), so this is set once here and then left to
  // the browser's own native editing until save reads it back out.
  await nextTick();
  if (editorEl.value) editorEl.value.innerHTML = DOMPurify.sanitize(html.value);
}

async function insertImage(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  try {
    const dataUrl = await fileToDataUrl(file);
    const img = document.createElement("img");
    img.src = dataUrl;
    img.dataset.filename = file.name;
    img.alt = file.name;
    editorEl.value?.appendChild(img);
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (imageInput.value) imageInput.value.value = "";
  }
}

async function save() {
  if (itemId.value === null || !editorEl.value) return;
  saving.value = true;
  saveError.value = null;
  try {
    // Every <img> currently in the edited content becomes one
    // `images` map entry, keyed by its own current `src` -- a fresh
    // data: URL for a just-inserted image, or the real
    // /get-note-resource/... URL already embedded in an existing
    // image's src for one being kept unchanged. The server does a
    // literal string-replace of each key against the submitted html
    // (see notes.rs::set_note's own doc), so using the src itself as
    // the key needs no separate placeholder-token scheme. An image
    // the user deleted from the editor simply has no <img> left to
    // scan, so it's dropped from `images` (and thus the note's
    // resources) automatically.
    const images: Record<string, NoteImageSpec> = {};
    for (const img of editorEl.value.querySelectorAll("img")) {
      const src = img.getAttribute("src");
      if (!src) continue;
      images[src] = { data: src, filename: img.dataset.filename };
    }
    // Sanitized again on the way out, not just on the way in -- a
    // browser paste can carry attacker-controlled markup (an
    // `onerror=` handler, a script) that survived into the live
    // contenteditable DOM despite the initial sanitize-on-load; this
    // is what actually keeps it out of the persisted note.
    const outgoingHtml = DOMPurify.sanitize(editorEl.value.innerHTML);
    const savedHtml = await saveNote(props.field, itemId.value, outgoingHtml, images);
    html.value = savedHtml;
    editing.value = false;
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="panel">
      <button class="close" @click="emit('close')">✕</button>
      <h3>{{ itemName }}</h3>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else>
        <div v-if="!editing" class="note-view" v-html="sanitizedHtml"></div>
        <div v-else ref="editorEl" class="note-view note-edit" contenteditable="true"></div>

        <p v-if="saveError" class="error">{{ saveError }}</p>

        <div class="actions">
          <template v-if="!editing">
            <button type="button" @click="startEditing">Edit note</button>
          </template>
          <template v-else>
            <button type="button" @click="imageInput?.click()">Insert image…</button>
            <input ref="imageInput" type="file" accept="image/*" class="hidden-file-input" @change="insertImage" />
            <button type="button" class="read" :disabled="saving" @click="save">{{ saving ? "Saving…" : "Save" }}</button>
            <button type="button" :disabled="saving" @click="editing = false">Cancel</button>
          </template>
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
  background: var(--bg);
  border-radius: 6px;
  padding: 1.5em;
  max-width: 640px;
  width: 90%;
  max-height: 85vh;
  overflow: auto;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1em;
}
.close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: var(--fs-medium);
  cursor: pointer;
}
h3 {
  margin: 0;
}
.note-view {
  min-height: 6em;
  line-height: 1.5;
}
.note-view :deep(img) {
  max-width: 100%;
}
.note-edit {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.5em;
}
.note-edit:focus {
  outline: 2px solid var(--accent);
}
.hidden-file-input {
  display: none;
}
.actions {
  display: flex;
  gap: 0.5em;
  flex-wrap: wrap;
}
.read {
  background: var(--accent);
  color: var(--fg-on-accent);
  border: none;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.error {
  color: var(--danger);
}
</style>
