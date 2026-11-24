<script setup lang="ts">
// The "Folders" category: the library's on-disk folder tree, with book counts
// that include sub-folders. Click a folder to filter the list to it (and what
// is below it); right-click for New subfolder, Move selected books here,
// Rename folder; drop selected rows from the table on a folder to move them.
// The logic (tree, query, drag payload, move flow) is in ../library/folders.ts.

import { computed, ref, watch } from "vue";

import { createLibraryFolder, deleteLibraryFolder, fetchLibraryFolders, type LibraryFolder } from "../library/api";
import {
  ancestorsOf, buildFolderTree, createFolderFlow, describeFolderPlan, describeFolderReport, folderNameProblem, folderQuery, FOLDERS_EXPANDED_KEY,
  hasBooksDrag, joinFolder, parseExpandedFolders, readBooksDrag, renamedPath, toggleFolder, visibleFolderRows,
} from "../library/folders";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import { getItemCompat } from "../library/legacyStorage";

const vFocus = { mounted: (el: HTMLElement) => el.focus() };

const props = defineProps<{ selectedIds: number[]; refreshKey: number }>();
const emit = defineEmits<{ select: [query: string, label: string]; changed: [] }>();

const folders = ref<LibraryFolder[]>([]);
const total = ref(0);
const error = ref<string | null>(null);
const open = ref(true);
const expanded = ref<Set<string>>(readExpanded());
const activePath = ref<string | null>(null);

function readExpanded(): Set<string> {
  try {
    return parseExpandedFolders(getItemCompat(localStorage, FOLDERS_EXPANDED_KEY));
  } catch {
    return new Set();
  }
}
function persistExpanded() {
  try {
    localStorage.setItem(FOLDERS_EXPANDED_KEY, JSON.stringify([...expanded.value]));
  } catch {
    // Expansion simply will not survive a restart.
  }
}

async function load() {
  try {
    const r = await fetchLibraryFolders();
    folders.value = r.folders;
    total.value = r.total;
    error.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
void load();
watch(() => props.refreshKey, () => void load());

const roots = computed(() => buildFolderTree(folders.value));
const rows = computed(() => visibleFolderRows(roots.value, expanded.value));

function toggle(path: string) {
  expanded.value = toggleFolder(expanded.value, path);
  persistExpanded();
}

function pick(path: string) {
  activePath.value = path;
  emit("select", folderQuery(path), path);
}

// --- the move / rename flow ---

const flow = createFolderFlow(undefined, async () => {
  await load();
  emit("changed");
});
const state = flow.state;
const flowOpen = computed(() => state.action !== null && (state.phase !== "idle" || state.error !== null || state.undone !== null));

// --- prompts: New subfolder, Rename folder ---

const prompt = ref<{ kind: "new" | "rename"; path: string; value: string; error: string | null; busy: boolean } | null>(null);

function askNew(parent: string) {
  prompt.value = { kind: "new", path: parent, value: "", error: null, busy: false };
}
function askRename(path: string) {
  prompt.value = { kind: "rename", path, value: path.slice(path.lastIndexOf("/") + 1), error: null, busy: false };
}

async function submitPrompt() {
  const p = prompt.value;
  if (!p) return;
  const problem = folderNameProblem(p.value);
  if (problem) {
    p.error = problem;
    return;
  }
  if (p.kind === "new") {
    p.busy = true;
    try {
      const made = await createLibraryFolder(joinFolder(p.path, p.value));
      prompt.value = null;
      expanded.value = new Set([...expanded.value, ...ancestorsOf(made.path)]);
      persistExpanded();
      await load();
    } catch (e) {
      p.error = e instanceof Error ? e.message : String(e);
      p.busy = false;
    }
    return;
  }
  const to = renamedPath(p.path, p.value);
  prompt.value = null;
  await flow.begin({ kind: "rename", from: p.path, to });
}

// --- menu ---

type MenuId = "show" | "new" | "move" | "rename" | "remove";
const menu = ref<{ x: number; y: number; path: string } | null>(null);
const menuEntries = computed<ContextMenuEntry<MenuId>[]>(() => {
  const target = menu.value;
  if (!target) return [];
  const folder = folders.value.find((f) => f.path === target.path);
  const n = props.selectedIds.length;
  return [
    { id: "show", label: "Show books in this folder", enabled: true },
    { id: "new", label: "New subfolder…", enabled: true, startsGroup: true },
    { id: "move", label: n > 0 ? `Move ${n} selected book${n === 1 ? "" : "s"} here` : "Move selected books here (none selected)", enabled: n > 0 },
    { id: "rename", label: "Rename folder…", enabled: true },
    { id: "remove", label: "Remove empty folder", enabled: folder?.books === 0, startsGroup: true },
  ];
});

async function onChoose(id: MenuId) {
  const target = menu.value;
  menu.value = null;
  if (!target) return;
  if (id === "show") pick(target.path);
  else if (id === "new") askNew(target.path);
  else if (id === "rename") askRename(target.path);
  else if (id === "move") await flow.begin({ kind: "move", ids: [...props.selectedIds], folder: target.path });
  else {
    try {
      await deleteLibraryFolder(target.path);
      await load();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }
}

// --- drop ---

const dropTarget = ref<string | null>(null);

function onDragOver(path: string, event: DragEvent) {
  if (!event.dataTransfer || !hasBooksDrag(event.dataTransfer)) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "move";
  dropTarget.value = path;
}

async function onDrop(path: string, event: DragEvent) {
  dropTarget.value = null;
  if (!event.dataTransfer || !hasBooksDrag(event.dataTransfer)) return;
  event.preventDefault();
  const ids = readBooksDrag(event.dataTransfer);
  if (ids.length > 0) await flow.begin({ kind: "move", ids, folder: path });
}

async function runFlow() {
  await flow.run();
}
function closeFlow() {
  flow.reset();
}

function planTitle(): string {
  const a = state.action;
  if (!a) return "";
  return a.kind === "move" ? `Move books to ${a.folder}` : `Rename folder ${a.from}`;
}
</script>

<template>
  <section class="folders" aria-label="Folders">
    <div class="folders-head">
      <button type="button" class="folders-toggle" :aria-expanded="open" @click="open = !open">
        <span class="twisty" aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
        <span class="folders-name">Folders</span>
        <span class="count">{{ total }}</span>
      </button>
      <button type="button" class="folders-add" title="New top-level folder" aria-label="New top-level folder" data-testid="new-root-folder" @click="askNew('')">+</button>
    </div>
    <p v-if="error" class="error">{{ error }}</p>
    <ul v-if="open" class="folder-list" data-testid="folder-list">
      <li v-if="rows.length === 0" class="folder-note">No folders yet. Books at the library's top level are not in a folder.</li>
      <li v-for="row in rows" :key="row.node.path">
        <div
          class="folder-row"
          :class="{ active: activePath === row.node.path, drop: dropTarget === row.node.path }"
          :style="{ paddingLeft: `calc(var(--sp-3) + ${row.depth * 12}px)` }"
          :data-folder="row.node.path"
          @dragover="onDragOver(row.node.path, $event)"
          @dragleave="dropTarget = null"
          @drop="onDrop(row.node.path, $event)"
          @contextmenu.prevent="menu = { x: $event.clientX, y: $event.clientY, path: row.node.path }"
        >
          <button v-if="row.node.children.length > 0" type="button" class="twisty-btn" :aria-label="expanded.has(row.node.path) ? 'Collapse' : 'Expand'" @click.stop="toggle(row.node.path)">{{ expanded.has(row.node.path) ? "▾" : "▸" }}</button>
          <span v-else class="twisty-btn spacer" aria-hidden="true"></span>
          <button type="button" class="folder-pick" :title="`${row.node.path} — ${row.node.books} book${row.node.books === 1 ? '' : 's'}`" @click="pick(row.node.path)">
            <span class="folder-label" :class="{ empty: row.node.empty }">{{ row.node.name }}</span>
            <span class="count">{{ row.node.books }}</span>
          </button>
        </div>
      </li>
    </ul>

    <ContextMenu v-if="menu" :x="menu.x" :y="menu.y" :entries="menuEntries" @choose="onChoose" @close="menu = null" />

    <div v-if="prompt" class="folder-backdrop" @click.self="prompt = null">
      <form class="folder-dialog" role="dialog" aria-modal="true" :aria-label="prompt.kind === 'new' ? 'New subfolder' : 'Rename folder'" @submit.prevent="submitPrompt">
        <h3>{{ prompt.kind === "new" ? (prompt.path ? `New subfolder in ${prompt.path}` : "New folder") : `Rename ${prompt.path}` }}</h3>
        <input v-model="prompt.value" type="text" v-focus data-testid="folder-name" spellcheck="false" placeholder="Folder name" />
        <p v-if="prompt.kind === 'new'" class="hint">An empty folder is made on disk and stays in the tree until you remove it.</p>
        <p v-else class="hint">Every book in it moves, with its sub-folders and file names. You will see the plan first, and can undo it.</p>
        <p v-if="prompt.error" class="error" role="alert">{{ prompt.error }}</p>
        <div class="buttons">
          <button type="submit" :disabled="prompt.busy" data-testid="folder-name-ok">{{ prompt.kind === "new" ? "Create" : "Preview" }}</button>
          <button type="button" @click="prompt = null">Cancel</button>
        </div>
      </form>
    </div>

    <div v-if="flowOpen && state.action" class="folder-backdrop" @click.self="state.phase !== 'running' && closeFlow()">
      <div class="folder-dialog" role="dialog" aria-modal="true" :aria-label="planTitle()" data-testid="folder-flow">
        <h3>{{ planTitle() }}</h3>
        <template v-if="state.plan && (state.phase === 'confirm' || (state.phase === 'running' && !state.report))">
          <p role="status" data-testid="folder-plan">{{ describeFolderPlan(state.action, state.plan) }}</p>
          <ul class="moves">
            <li v-for="(m, i) in state.plan.moves.slice(0, 8)" :key="i"><span class="path">{{ m.from }} → {{ m.to }}</span></li>
          </ul>
          <p v-if="state.plan.moves.length > 8" class="hint">…and {{ (state.plan.files - 8).toLocaleString("en-US") }} more files.</p>
          <p class="hint">Nothing is overwritten (a name already taken gets a number), and the run can be undone.</p>
        </template>
        <p v-if="state.phase === 'running'" class="hint" role="status">{{ state.progress }}</p>
        <p v-if="state.report && state.phase !== 'running'" role="status" data-testid="folder-report">{{ describeFolderReport(state.report) }}</p>
        <p v-if="state.undone" role="status">{{ state.undone }}</p>
        <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>
        <div class="buttons">
          <button v-if="state.phase === 'confirm'" type="button" data-testid="folder-run" @click="runFlow">{{ state.action.kind === "move" ? "Move" : "Rename" }} {{ state.plan?.books.toLocaleString("en-US") }} book{{ state.plan?.books === 1 ? "" : "s" }}</button>
          <button v-if="state.phase === 'done'" type="button" data-testid="folder-undo" @click="flow.undo()">Undo</button>
          <button type="button" :disabled="state.phase === 'running'" @click="closeFlow">{{ state.phase === "done" || state.undone ? "Close" : "Cancel" }}</button>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.folders {
  border-bottom: 1px solid var(--border);
  max-height: 45%;
  overflow-y: auto;
  flex-shrink: 0;
}
.folders-toggle {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  width: 100%;
  height: 22px;
  padding: 0 var(--sp-3);
  border: none;
  border-radius: 0;
  background: none;
  text-align: left;
  font-size: var(--fs-body);
  font-weight: 600;
}
.folders-toggle:hover,
.folder-row:hover {
  background: var(--bg-hover);
}
.twisty {
  flex: 0 0 10px;
  font-size: var(--fs-micro);
  color: var(--fg-faint);
}
.folders-name {
  flex: 1;
}
.folders-head {
  display: flex;
  align-items: center;
}
.folders-head .folders-toggle {
  flex: 1;
}
.folders-add {
  height: 22px;
  padding: 0 var(--sp-3);
  border: none;
  background: none;
  color: var(--fg-muted);
  cursor: pointer;
}
.folders-add:hover {
  color: var(--fg);
}
.folder-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.folder-row {
  display: flex;
  align-items: center;
  height: 22px;
  padding-right: var(--sp-3);
}
.folder-row.active {
  background: var(--accent-soft);
}
.folder-row.drop {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
  background: var(--accent-soft);
}
.twisty-btn {
  flex: 0 0 14px;
  height: 20px;
  padding: 0;
  border: none;
  background: none;
  color: var(--fg-faint);
  font-size: var(--fs-micro);
}
.twisty-btn.spacer {
  display: inline-block;
}
.folder-pick {
  display: flex;
  flex: 1;
  min-width: 0;
  align-items: center;
  gap: var(--sp-2);
  height: 22px;
  padding: 0 0 0 var(--sp-1);
  border: none;
  background: none;
  text-align: left;
  font-size: var(--fs-body);
}
.folder-label {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.folder-label.empty {
  font-style: italic;
  color: var(--fg-muted);
}
.count {
  font-size: var(--fs-micro);
  color: var(--fg-faint);
  font-variant-numeric: tabular-nums;
}
.folder-note {
  padding: var(--sp-1) var(--sp-3);
  font-size: var(--fs-small);
  color: var(--fg-faint);
}
.error {
  padding: var(--sp-1) var(--sp-3);
  margin: 0;
  color: var(--danger);
  font-size: var(--fs-small);
}
.folder-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 30;
}
.folder-dialog {
  background: var(--bg);
  border-radius: 6px;
  padding: 1.1rem 1.4rem;
  width: min(560px, 94vw);
  max-height: 80vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 0.7rem;
}
.folder-dialog h3 {
  margin: 0;
}
.hint {
  margin: 0;
  color: var(--fg-muted);
  font-size: var(--fs-body);
}
.moves {
  margin: 0;
  padding-left: 1.1rem;
}
.path {
  font-family: monospace;
  font-size: var(--fs-body);
  overflow-wrap: anywhere;
  color: var(--fg-muted);
}
.buttons {
  display: flex;
  gap: 0.5rem;
}
</style>
