<script setup lang="ts">
// Virtual libraries and saved searches: list, add, rename (saved searches) and
// delete. The lists are the library view's; this reports each change and the
// view reloads them. Real, new routes (see crates/lexmechanic_srv/src/lists.rs's
// own doc for why no upstream route exists to port here).

import { ref } from "vue";

import { deleteSavedSearch, deleteVirtualLibrary, renameSavedSearch, setSavedSearch, setVirtualLibrary } from "../../library/api";
import Dialog from "../Dialog.vue";
import { confirmDialog, promptDialog } from "./confirm";

const props = defineProps<{
  virtualLibraries: Record<string, string>;
  savedSearches: Record<string, string>;
  /** The committed search, offered as the query of a new list. */
  query: string;
}>();
const emit = defineEmits<{
  close: [];
  /** A list changed on the server: reload the lists. */
  changed: [];
  /** A virtual library was deleted (it may be the one in use). */
  "virtual-library-deleted": [name: string];
}>();

const manageError = ref<string | null>(null);
const newVlName = ref("");
const newVlQuery = ref(props.query);
const newSearchName = ref("");
const newSearchQuery = ref(props.query);

async function createVirtualLibrary() {
  if (!newVlName.value.trim() || !newVlQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setVirtualLibrary(newVlName.value.trim(), newVlQuery.value.trim());
    newVlName.value = "";
    emit("changed");
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeVirtualLibrary(name: string) {
  if (!(await confirmDialog(`Delete the virtual library "${name}"?`, { confirmLabel: "Delete", danger: true }))) return;
  manageError.value = null;
  try {
    await deleteVirtualLibrary(name);
    emit("virtual-library-deleted", name);
    emit("changed");
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function createSavedSearch() {
  if (!newSearchName.value.trim() || !newSearchQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setSavedSearch(newSearchName.value.trim(), newSearchQuery.value.trim());
    newSearchName.value = "";
    emit("changed");
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeSavedSearch(name: string) {
  if (!(await confirmDialog(`Delete the saved search "${name}"?`, { confirmLabel: "Delete", danger: true }))) return;
  manageError.value = null;
  try {
    await deleteSavedSearch(name);
    emit("changed");
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function renameSavedSearchPrompt(name: string) {
  const newName = await promptDialog("Rename saved search to:", name, { title: "Rename saved search", confirmLabel: "Rename" });
  if (!newName || newName === name) return;
  manageError.value = null;
  try {
    await renameSavedSearch(name, newName);
    emit("changed");
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <Dialog label="Virtual libraries and saved searches" show-close size="lg" @close="emit('close')">
    <p v-if="manageError" class="error">{{ manageError }}</p>

    <section>
      <h3>Virtual libraries</h3>
      <ul class="manage-list">
        <li v-for="[name, q] in Object.entries(virtualLibraries)" :key="name">
          <span class="manage-name">{{ name }}</span>
          <code class="manage-query">{{ q }}</code>
          <button type="button" class="manage-remove" @click="removeVirtualLibrary(name)">Delete</button>
        </li>
      </ul>
      <form class="manage-form" @submit.prevent="createVirtualLibrary">
        <input v-model="newVlName" placeholder="Name" required />
        <input v-model="newVlQuery" placeholder="Search query" required />
        <button type="submit">Add</button>
      </form>
    </section>

    <section>
      <h3>Saved searches</h3>
      <ul class="manage-list">
        <li v-for="[name, q] in Object.entries(savedSearches)" :key="name">
          <span class="manage-name">{{ name }}</span>
          <code class="manage-query">{{ q }}</code>
          <button type="button" @click="renameSavedSearchPrompt(name)">Rename</button>
          <button type="button" class="manage-remove" @click="removeSavedSearch(name)">Delete</button>
        </li>
      </ul>
      <form class="manage-form" @submit.prevent="createSavedSearch">
        <input v-model="newSearchName" placeholder="Name" required />
        <input v-model="newSearchQuery" placeholder="Search query" required />
        <button type="submit">Add</button>
      </form>
    </section>
  </Dialog>
</template>

<style scoped>
.error {
  padding: 1em;
  color: var(--danger);
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.manage-query {
  color: var(--fg-muted);
  font-size: var(--fs-small);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.manage-remove {
  color: var(--danger);
}
</style>
