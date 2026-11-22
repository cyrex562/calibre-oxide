<script setup lang="ts">
// "File naming…": the library's naming convention. A folder template and a file
// template, a live preview on a few books, a short token reference, and an
// "Apply" button that opens Organize with the convention filled in (so the dry
// run, the run and the undo all come from there). State: ../library/naming.ts.

import { onBeforeUnmount, onMounted, watch } from "vue";

import { combinedTemplate, NAMING_TOKENS, type NamingController } from "../library/naming";

const props = defineProps<{ controller: NamingController; sampleIds: number[]; selectedCount: number }>();
const emit = defineEmits<{ close: []; apply: [template: string, scope: "selected" | "all"] }>();

const c = props.controller;
const state = c.state;

let timer: ReturnType<typeof setTimeout> | null = null;
// Edits preview after a short pause, so typing is not a request per key.
watch(
  () => [state.folder, state.file],
  () => {
    if (!state.loaded) return;
    state.note = "";
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void c.preview(), 250);
  },
);
onMounted(() => void c.load(props.sampleIds));
onBeforeUnmount(() => timer && clearTimeout(timer));

async function saveAndApply(scope: "selected" | "all") {
  if (c.dirty() && !(await c.save())) return;
  emit("apply", combinedTemplate(state.folder, state.file), scope);
}
</script>

<template>
  <div class="naming-backdrop" @click.self="emit('close')">
    <div class="naming-panel" role="dialog" aria-label="File naming">
      <button class="naming-close" aria-label="Close" @click="emit('close')">✕</button>
      <h3>File naming</h3>
      <p class="hint">Where this library's files live and what they are called. Organize, Rename files and Fix titles start from this; any run can still use something else.</p>

      <label class="field">
        Folder
        <input v-model="state.folder" type="text" data-testid="folder-template" spellcheck="false" placeholder="e.g. Gaming/RPG/{#game_line}" />
        <span class="hint">Leave empty to keep every file at the top of the library.</span>
      </label>
      <label class="field">
        File name (the extension is added)
        <input v-model="state.file" type="text" data-testid="file-template" spellcheck="false" placeholder="e.g. {series:|| }{title}" />
      </label>
      <div class="buttons">
        <button type="button" data-testid="use-example" @click="c.useExample()">Use the RPG example</button>
        <button type="button" @click="c.resetToDefaults()">Reset to the default</button>
      </div>

      <p v-if="state.error" class="error" role="alert" data-testid="naming-error">{{ state.error }}</p>

      <section class="preview" aria-label="Preview">
        <h4>Preview {{ sampleIds.length > 0 ? "(selected books)" : "(sample books)" }}</h4>
        <p v-if="state.previews.length === 0 && !state.error" class="hint">No books to preview yet.</p>
        <ul data-testid="naming-preview">
          <li v-for="p in state.previews" :key="p.book_id">
            <span class="path">{{ p.path ?? "—" }}</span>
            <span v-if="p.error" class="error"> {{ p.error }}</span>
            <span class="hint"> {{ p.title }}</span>
          </li>
        </ul>
      </section>

      <details class="tokens" open>
        <summary>Template reference</summary>
        <div v-for="g in NAMING_TOKENS" :key="g.title" class="token-group">
          <h4>{{ g.title }}</h4>
          <dl>
            <template v-for="r in g.rows" :key="r.token">
              <dt><code>{{ r.token }}</code></dt>
              <dd>{{ r.meaning }}</dd>
            </template>
          </dl>
        </div>
        <p class="hint">A book with no value for a token leaves no empty folder and no stray separator. A name already taken gets a number.</p>
      </details>

      <div class="buttons">
        <button type="button" :disabled="state.saving || !c.dirty() || c.problem() !== null" data-testid="save-naming" @click="c.save()">{{ state.saving ? "Saving…" : "Save" }}</button>
        <button type="button" :disabled="state.saving || c.problem() !== null || selectedCount === 0" data-testid="apply-selected" @click="saveAndApply('selected')">Apply to the {{ selectedCount }} selected book{{ selectedCount === 1 ? "" : "s" }}…</button>
        <button type="button" :disabled="state.saving || c.problem() !== null" data-testid="apply-all" @click="saveAndApply('all')">Apply to all books…</button>
      </div>
      <p v-if="state.note" class="hint" role="status">{{ state.note }}</p>
      <p class="hint">“Apply” opens Organize with this convention filled in: you see the dry run first, and can undo the run.</p>
    </div>
  </div>
</template>

<style scoped>
.naming-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 20;
}
.naming-panel {
  position: relative;
  background: var(--bg);
  border-radius: 6px;
  padding: 1.25rem 1.5rem;
  width: min(760px, 94vw);
  max-height: 90vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 0.8rem;
}
.naming-panel h3,
.naming-panel h4 {
  margin: 0;
}
.naming-close {
  position: absolute;
  top: 0.4rem;
  right: 0.5rem;
  border: none;
  background: none;
  cursor: pointer;
  font-size: var(--fs-medium);
}
.hint {
  color: var(--fg-muted);
  font-size: var(--fs-body);
  margin: 0;
}
.error {
  color: var(--error, #cf222e);
  margin: 0;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}
.field input {
  font-family: monospace;
}
.buttons {
  display: flex;
  gap: 0.5rem;
  flex-wrap: wrap;
}
.preview {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.6rem 0.8rem;
}
.preview ul {
  margin: 0.3rem 0 0;
  padding-left: 1.1rem;
}
.path {
  font-family: monospace;
  overflow-wrap: anywhere;
}
.tokens dl {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 0.15rem 1rem;
  margin: 0.2rem 0 0.5rem;
}
.tokens dd {
  margin: 0;
  color: var(--fg-muted);
}
</style>
