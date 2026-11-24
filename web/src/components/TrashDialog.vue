<script setup lang="ts">
// The library's trash (calibre's `.caltrash`): books and formats that were
// deleted, with Restore, Delete permanently and Empty trash.
//
// The wording and arithmetic live in ../library/trash.ts. Deleting for good
// and emptying both ask first; restoring does not, since it undoes a deletion.

import Dialog from "./Dialog.vue";
import { confirmDialog } from "./dialogs/confirm";
import { computed, onMounted, ref } from "vue";

import { deleteTrashEntry, emptyTrash, fetchTrash, restoreTrashEntry, type RestoreResult } from "../library/api";
import { confirmDeleteForever, confirmEmpty, describeKind, describeRestored, describeTrash, entryId, formatDeleted, formatSize, restoreRequests, sortEntries, timeLeft, type TrashEntry, type TrashListing } from "../library/trash";

const emit = defineEmits<{ close: []; changed: [] }>();

const listing = ref<TrashListing | null>(null);
const selected = ref<Set<string>>(new Set());
const busy = ref(false);
const error = ref<string | null>(null);
const status = ref<string | null>(null);

const entries = computed(() => sortEntries(listing.value?.entries ?? []));
const chosen = computed(() => entries.value.filter((e) => selected.value.has(entryId(e))));
const nowSeconds = ref(Math.floor(Date.now() / 1000));

async function load() {
  try {
    listing.value = await fetchTrash();
    nowSeconds.value = Math.floor(Date.now() / 1000);
    // Drop selections of rows that are gone.
    const ids = new Set(listing.value.entries.map(entryId));
    selected.value = new Set([...selected.value].filter((id) => ids.has(id)));
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

onMounted(load);

function toggle(entry: TrashEntry) {
  const next = new Set(selected.value);
  const id = entryId(entry);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selected.value = next;
}

function toggleAll() {
  selected.value = selected.value.size === entries.value.length ? new Set() : new Set(entries.value.map(entryId));
}

async function run(work: () => Promise<string | null>) {
  busy.value = true;
  error.value = null;
  status.value = null;
  try {
    status.value = await work();
    emit("changed");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
    await load();
  }
}

function restore() {
  const targets = chosen.value.filter((e) => e.problem === null);
  void run(async () => {
    const results: RestoreResult[] = [];
    for (const entry of targets) {
      for (const request of restoreRequests(entry)) results.push(await restoreTrashEntry(request));
    }
    return describeRestored(results);
  });
}

async function deleteForever() {
  const targets = chosen.value;
  if (targets.length === 0 || !(await confirmDialog(confirmDeleteForever(targets), { confirmLabel: "Delete permanently", danger: true }))) return;
  void run(async () => {
    for (const entry of targets) await deleteTrashEntry({ kind: entry.kind, key: entry.key });
    return `Permanently deleted ${targets.length} item${targets.length === 1 ? "" : "s"}.`;
  });
}

async function empty() {
  if (!listing.value || listing.value.count === 0 || !(await confirmDialog(confirmEmpty(listing.value), { confirmLabel: "Empty the trash", danger: true }))) return;
  void run(async () => {
    const { removed } = await emptyTrash();
    return `Emptied the trash (${removed} item${removed === 1 ? "" : "s"}).`;
  });
}
</script>

<template>
  <Dialog title="Trash" :busy="busy" size="lg" @close="emit('close')">
    <p class="hint">
      Books and formats you removed from the library, kept inside it so they can be put back.
      <template v-if="listing"> {{ describeTrash(listing) }}</template>
    </p>

    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="status" class="status" role="status">{{ status }}</p>

    <div v-if="listing && entries.length" class="trash-table-wrap">
      <table class="trash-table">
        <thead>
          <tr>
            <th><input type="checkbox" :checked="selected.size === entries.length" aria-label="Select all" @change="toggleAll" /></th>
            <th>Title</th>
            <th>Author</th>
            <th>What</th>
            <th>Size</th>
            <th>Deleted</th>
            <th>Expires</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="entry in entries" :key="entryId(entry)" :class="{ chosen: selected.has(entryId(entry)) }" @click="toggle(entry)">
            <td><input type="checkbox" :checked="selected.has(entryId(entry))" :aria-label="`Select ${entry.title}`" @click.stop @change="toggle(entry)" /></td>
            <td class="trash-title" :title="entry.original_path">{{ entry.title }}</td>
            <td>{{ entry.authors }}</td>
            <td>{{ describeKind(entry) }}</td>
            <td class="num">{{ formatSize(entry.size) }}</td>
            <td>{{ formatDeleted(entry) }}</td>
            <td :title="entry.problem ?? ''">{{ entry.problem ? "can’t restore here" : timeLeft(entry, nowSeconds) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="listing" class="hint">Nothing here.</p>
    <p v-else-if="!error" class="hint">Loading…</p>

    <template #footer>
      <button type="button" :disabled="busy || chosen.length === 0 || chosen.every((e) => e.problem !== null)" @click="restore">Restore{{ chosen.length ? ` (${chosen.length})` : "" }}</button>
      <button type="button" :disabled="busy || chosen.length === 0" @click="deleteForever">Delete permanently…</button>
      <button type="button" :disabled="busy || !listing || listing.count === 0" @click="empty">Empty trash…</button>
      <span class="trash-spacer"></span>
      <button type="button" class="primary" :disabled="busy" @click="emit('close')">Close</button>
    </template>
  </Dialog>
</template>

<style scoped>
.trash-table-wrap {
  max-height: 50vh;
  overflow: auto;
  border: 1px solid var(--border);
}
.trash-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--fs-small);
}
.trash-table th,
.trash-table td {
  text-align: left;
  padding: 0.25rem 0.5rem;
  border-bottom: 1px solid var(--border);
  white-space: nowrap;
}
.trash-table .trash-title {
  max-width: 28ch;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 600;
}
.trash-table .num {
  text-align: right;
}
.trash-table tbody tr {
  cursor: pointer;
}
.trash-table tbody tr.chosen {
  background: var(--bg-selected);
}
.trash-spacer {
  flex: 1;
}
</style>
