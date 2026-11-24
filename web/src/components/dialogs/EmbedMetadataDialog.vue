<script setup lang="ts">
// Embed metadata into the book files themselves (#958). The counterpart to
// "Rename files": that changes the name on disk, this changes the metadata
// *inside* each file, so a reader app shows the title this library has rather
// than the one the file shipped with.

import { ref } from "vue";

import { embedMetadata, summarizeEmbed } from "../../library/api";
import Dialog from "../Dialog.vue";

const props = defineProps<{ ids: number[] }>();
const emit = defineEmits<{
  close: [];
  /** Files were rewritten, so their sizes changed: the list is stale. */
  embedded: [];
}>();

const embedBusy = ref(false);
const embedDone = ref(false);
const embedCount = ref(0);
const embedProblems = ref<string[]>([]);
const embedError = ref<string | null>(null);

/**
 * Deliberately behind a confirm step rather than firing on click: this
 * rewrites the user's actual book files, which is not something to do
 * on a mis-click, and unlike most actions here it cannot be undone.
 */
async function runEmbedMetadata() {
  const ids = props.ids;
  if (ids.length === 0) return;
  embedBusy.value = true;
  embedError.value = null;
  embedProblems.value = [];
  try {
    const summary = summarizeEmbed(await embedMetadata(ids));
    embedCount.value = summary.embedded;
    embedProblems.value = summary.problems;
    embedDone.value = true;
    // Each rewritten file has a new size on disk, and `embed_metadata`
    // updates the `data` row to match, so the sizes in the grid are
    // stale until this reloads them.
    emit("embedded");
  } catch (e) {
    embedError.value = e instanceof Error ? e.message : String(e);
  } finally {
    embedBusy.value = false;
  }
}
</script>

<template>
    <Dialog title="Embed metadata in files" show-close size="lg" @close="emit('close')">
      <p class="news-hint">
        Writes this library's metadata into the selected book file(s), so another reader app shows the title, authors and
        cover recorded here rather than whatever the file shipped with. This rewrites the files themselves and cannot be undone.
        Formats with no metadata writer yet are listed below and left untouched.
      </p>
      <template v-if="embedDone">
        <p class="manage-name">{{ embedCount }} file(s) updated.</p>
        <ul v-if="embedProblems.length" class="manage-list">
          <li v-for="(problem, i) in embedProblems" :key="i"><span class="error">{{ problem }}</span></li>
        </ul>
      </template>
      <p v-if="embedError" class="error">{{ embedError }}</p>
      <template #footer>
        <template v-if="!embedDone">
          <button type="button" :disabled="embedBusy" @click="emit('close')">Cancel</button>
          <button type="button" class="primary" :disabled="embedBusy" @click="runEmbedMetadata">
            {{ embedBusy ? "Embedding…" : `Embed into ${ids.length} book(s)` }}
          </button>
        </template>
        <button v-else type="button" class="primary" @click="emit('close')">Close</button>
      </template>
    </Dialog>
</template>

<style scoped>
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.error {
  padding: 1em;
  color: var(--danger);
}
.news-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-size: var(--fs-small);
  color: var(--fg-muted);
  margin-bottom: 0.75em;
}
.news-field input,
.news-field textarea {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: 4px;
  resize: vertical;
}
</style>
