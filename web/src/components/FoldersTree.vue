<script setup lang="ts">
// The "Folders" category: the library's on-disk folder tree, with book counts
// that include sub-folders. Click a folder to filter the list to it (and what
// is below it); right-click for New subfolder, Move selected books here,
// Rename folder; drop selected rows from the table on a folder to move them.
// The logic (tree, query, drag payload, move flow) is in ../library/folders.ts.
//
// A share can have thousands of folders, so the visible (expanded) tree is
// flattened to a row list of fixed height and only the rows in the scroll
// window are drawn, between two spacers (../library/virtualList.ts).

import { useKeepScroll } from "../library/keepScroll";
import Dialog from "./Dialog.vue";
import { computed, nextTick, onActivated, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { createLibraryFolder, deleteLibraryFolder, fetchLibraryFolders, undoOrganize, type LibraryFolder } from "../library/api";
import { showToast } from "../library/deleteFlow";
import {
  ancestorsOf, buildFolderTree, createFolderFlow, describeFolderPlan, describeFolderReport, describeFolderUndo, describeQuickMove, folderNameProblem, folderQuery, FOLDERS_EXPANDED_KEY,
  hasBooksDrag, joinFolder, parseExpandedFolders, readBooksDrag, renamedPath, toggleFolder, visibleFolderRows,
} from "../library/folders";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import { getItemCompat } from "../library/legacyStorage";
import { scrollTopForIndex, windowRange } from "../library/virtualList";

/** Height of one folder row; the `.folder-row` rule says the same. */
const ROW_HEIGHT = 22;
/**
 * The smallest viewport the window is computed for. The section is as tall as its
 * rows (up to a cap), so its measured height lags behind the rows it is about to
 * get; assuming at least this much keeps a freshly expanded folder from showing
 * a few rows and then filling in. It is also the answer where nothing is laid out.
 */
const MIN_VIEWPORT = 440;

const rootEl = ref<HTMLElement | null>(null);
const listEl = ref<HTMLElement | null>(null);
useKeepScroll(rootEl);

const vFocus = { mounted: (el: HTMLElement) => el.focus() };

const props = defineProps<{ selectedIds: number[]; refreshKey: number }>();
const emit = defineEmits<{ select: [query: string, label: string]; changed: [] }>();

const folders = ref<LibraryFolder[]>([]);
const total = ref(0);
/** How many of calibre's author folders the library has, and whether the tree lists them. */
const authorFolders = ref(0);
const showAuthors = ref(readShowAuthors());
function readShowAuthors(): boolean {
  try {
    return localStorage.getItem("lexmechanic.folders.authors") === "1";
  } catch {
    return false;
  }
}
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
    const r = await fetchLibraryFolders(showAuthors.value);
    folders.value = r.folders;
    total.value = r.total;
    authorFolders.value = r.author_folders ?? 0;
    error.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
void load();
watch(() => props.refreshKey, () => void load());
watch(showAuthors, (on) => {
  try {
    localStorage.setItem("lexmechanic.folders.authors", on ? "1" : "0");
  } catch {
    // The choice simply will not survive a restart.
  }
  void load();
});

const roots = computed(() => buildFolderTree(folders.value));
const rows = computed(() => visibleFolderRows(roots.value, expanded.value));

// --- the window of rows that is drawn ---

const scrollTop = ref(0);
const viewportHeight = ref(0);
/** Where the list starts inside the scrolling section (the header sits above it). */
const listOffset = ref(0);

function measure() {
  const root = rootEl.value;
  if (!root) return;
  scrollTop.value = root.scrollTop;
  viewportHeight.value = root.clientHeight;
  const list = listEl.value;
  listOffset.value = list ? list.getBoundingClientRect().top - root.getBoundingClientRect().top + root.scrollTop : 0;
}

const windowed = computed(() =>
  windowRange({ count: rows.value.length, rowHeight: ROW_HEIGHT, scrollTop: Math.max(0, scrollTop.value - listOffset.value), viewportHeight: Math.max(viewportHeight.value, MIN_VIEWPORT) }),
);
const visibleRows = computed(() => rows.value.slice(windowed.value.start, windowed.value.end));

let resizeObserver: ResizeObserver | null = null;
onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && rootEl.value) {
    resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(rootEl.value);
  }
});
onBeforeUnmount(() => resizeObserver?.disconnect());
onActivated(() => void nextTick(measure));
// Reopening the section puts the list back under the header, and a changed row
// count changes how tall the section is.
watch(open, () => void nextTick(measure));
watch(() => rows.value.length, () => void nextTick(measure));

/** Scrolls so the row for `path` is fully visible (nothing happens for a path that is not shown). */
function revealRow(path: string) {
  const root = rootEl.value;
  const index = rows.value.findIndex((r) => r.node.path === path);
  if (!root || index < 0) return;
  measure();
  const next = scrollTopForIndex(index, { count: rows.value.length, rowHeight: ROW_HEIGHT, scrollTop: Math.max(0, scrollTop.value - listOffset.value), viewportHeight: Math.max(0, viewportHeight.value - listOffset.value) });
  if (next !== Math.max(0, scrollTop.value - listOffset.value)) {
    root.scrollTop = next + listOffset.value;
    measure();
  }
}

// The current folder stays in view when the tree changes under it (expanding or
// collapsing above it, a refresh) and when it was chosen from elsewhere.
watch(activePath, (path) => {
  if (path) void nextTick(() => revealRow(path));
});

function toggle(path: string) {
  expanded.value = toggleFolder(expanded.value, path);
  persistExpanded();
}

function pick(path: string) {
  activePath.value = path;
  emit("select", folderQuery(path), path);
}

// --- keyboard: the ARIA tree pattern ---
//
// The tree is one Tab stop. The row that holds it (`tabStop`) is the one last
// focused, else the picked folder, else the first row; arrow keys move focus
// between rows and expand / collapse, Enter or Space picks. The twisty and name
// buttons inside a row are for the mouse only (tabindex -1), so Tab goes past
// the whole tree instead of stopping at every folder.

const focusedPath = ref<string | null>(null);

/** The row that is in the Tab order; always one that is drawn, so the tree can never be skipped. */
const tabStop = computed<string | null>(() => {
  const drawn = visibleRows.value;
  if (drawn.length === 0) return null;
  for (const wanted of [focusedPath.value, activePath.value]) {
    if (wanted && drawn.some((r) => r.node.path === wanted)) return wanted;
  }
  return drawn[0].node.path;
});

function rowElement(path: string): HTMLElement | null {
  const els = listEl.value?.querySelectorAll<HTMLElement>("[data-folder]") ?? [];
  for (const el of els) if (el.dataset.folder === path) return el;
  return null;
}

/** Moves keyboard focus to a row, scrolling it into the drawn window first. */
async function focusRow(path: string) {
  focusedPath.value = path;
  revealRow(path);
  await nextTick();
  rowElement(path)?.focus();
}

function parentOf(path: string): string | null {
  const cut = path.lastIndexOf("/");
  return cut < 0 ? null : path.slice(0, cut);
}

function onTreeKey(row: { node: { path: string; children: unknown[] } }, event: KeyboardEvent) {
  // Keys that reach the row from a control inside it (the buttons are not in
  // the Tab order, but a click focuses them) are not the row's to answer.
  if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return;
  const path = row.node.path;
  const list = rows.value;
  const at = list.findIndex((r) => r.node.path === path);
  const hasChildren = row.node.children.length > 0;
  const go = (index: number) => {
    const target = list[Math.min(list.length - 1, Math.max(0, index))];
    if (target) void focusRow(target.node.path);
  };
  switch (event.key) {
    case "ArrowDown":
      go(at + 1);
      break;
    case "ArrowUp":
      go(at - 1);
      break;
    case "Home":
      go(0);
      break;
    case "End":
      go(list.length - 1);
      break;
    case "ArrowRight":
      if (hasChildren && !expanded.value.has(path)) toggle(path);
      else if (hasChildren) go(at + 1);
      break;
    case "ArrowLeft": {
      const parent = parentOf(path);
      if (hasChildren && expanded.value.has(path)) toggle(path);
      else if (parent !== null) void focusRow(parent);
      break;
    }
    case "Enter":
    case " ":
      pick(path);
      break;
    case "ContextMenu":
    case "F10": {
      if (event.key === "F10" && !event.shiftKey) return;
      const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
      menu.value = { x: box.left + 24, y: box.bottom, path };
      break;
    }
    default:
      return;
  }
  event.preventDefault();
}

/** A click on a row's own buttons also puts keyboard focus on the row, so the arrows work from there. */
function focusOwnRow(event: Event, path: string) {
  focusedPath.value = path;
  (event.currentTarget as HTMLElement).closest<HTMLElement>("[role=treeitem]")?.focus();
}

// --- the move / rename flow ---

const flow = createFolderFlow(undefined, async () => {
  await load();
  emit("changed");
});
const state = flow.state;
// A quick move (a few books, no collisions) has no dialog: it is reported in a toast.
const flowOpen = computed(() => state.action !== null && !state.quiet && !(state.quick && state.phase === "done") && (state.phase !== "idle" || state.error !== null || state.undone !== null));

/** Moves books into a folder: at once and with an Undo toast when that is small and clean, else after a preview. */
async function beginMove(ids: number[], folder: string) {
  await flow.begin({ kind: "move", ids, folder }, { quick: true });
  const report = state.report;
  if (state.quick && state.phase === "done" && report) {
    flow.reset();
    showToast(describeQuickMove(report, folder), {
      label: "Undo",
      run: async () => {
        const done = await undoOrganize(report.run_id);
        await load();
        emit("changed");
        return describeFolderUndo(done);
      },
    });
  }
}

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
      await nextTick();
      revealRow(made.path);
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
  else if (id === "move") await beginMove([...props.selectedIds], target.path);
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
  if (ids.length > 0) await beginMove(ids, path);
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
  <section ref="rootEl" class="folders" aria-label="Folders" @scroll.passive="measure">
    <div class="folders-head">
      <button type="button" class="folders-toggle" :aria-expanded="open" @click="open = !open">
        <span class="twisty" aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
        <span class="folders-name">Folders</span>
        <span class="count">{{ total }}</span>
      </button>
      <button type="button" class="folders-add" title="New top-level folder" aria-label="New top-level folder" data-testid="new-root-folder" @click="askNew('')">+</button>
    </div>
    <p v-if="error" class="error">{{ error }}</p>
    <label v-if="open && authorFolders > 0" class="folders-authors" data-testid="author-folders-toggle" title="calibre keeps each author's books in a folder named for the author. Show them in this tree.">
      <input v-model="showAuthors" type="checkbox" />
      Author folders ({{ authorFolders.toLocaleString("en-US") }})
    </label>
    <ul v-if="open" ref="listEl" class="folder-list" :role="rows.length > 0 ? 'tree' : undefined" aria-label="Folders" data-testid="folder-list">
      <li v-if="rows.length === 0" class="folder-note">No folders yet. Books at the library's top level are not in a folder.</li>
      <li v-if="windowed.padTop > 0" aria-hidden="true" :style="{ height: `${windowed.padTop}px` }"></li>
      <li v-for="row in visibleRows" :key="row.node.path" role="none">
        <div
          class="folder-row"
          role="treeitem"
          :class="{ active: activePath === row.node.path, drop: dropTarget === row.node.path }"
          :style="{ paddingLeft: `calc(var(--sp-3) + ${row.depth * 12}px)` }"
          :data-folder="row.node.path"
          :tabindex="tabStop === row.node.path ? 0 : -1"
          :aria-level="row.depth + 1"
          :aria-expanded="row.node.children.length > 0 ? expanded.has(row.node.path) : undefined"
          :aria-selected="activePath === row.node.path"
          :aria-label="`${row.node.name}, ${row.node.books} book${row.node.books === 1 ? '' : 's'}`"
          @focus="focusedPath = row.node.path"
          @keydown="onTreeKey(row, $event)"
          @dragover="onDragOver(row.node.path, $event)"
          @dragleave="dropTarget = null"
          @drop="onDrop(row.node.path, $event)"
          @contextmenu.prevent="menu = { x: $event.clientX, y: $event.clientY, path: row.node.path }"
        >
          <button v-if="row.node.children.length > 0" type="button" class="twisty-btn" tabindex="-1" aria-hidden="true" :aria-label="expanded.has(row.node.path) ? 'Collapse' : 'Expand'" :title="expanded.has(row.node.path) ? 'Collapse' : 'Expand'" @click.stop="toggle(row.node.path); focusOwnRow($event, row.node.path)">{{ expanded.has(row.node.path) ? "▾" : "▸" }}</button>
          <span v-else class="twisty-btn spacer" aria-hidden="true"></span>
          <button type="button" class="folder-pick" tabindex="-1" aria-hidden="true" :title="`${row.node.path} — ${row.node.books} book${row.node.books === 1 ? '' : 's'}`" @click="pick(row.node.path); focusOwnRow($event, row.node.path)">
            <span class="folder-label" :class="{ empty: row.node.empty }">{{ row.node.name }}</span>
            <span class="count">{{ row.node.books }}</span>
          </button>
        </div>
      </li>
      <li v-if="windowed.padBottom > 0" aria-hidden="true" :style="{ height: `${windowed.padBottom}px` }"></li>
    </ul>

    <ContextMenu v-if="menu" :x="menu.x" :y="menu.y" :entries="menuEntries" @choose="onChoose" @close="menu = null" />

    <Dialog
      v-if="prompt"
      :title="prompt.kind === 'new' ? (prompt.path ? `New subfolder in ${prompt.path}` : 'New folder') : `Rename ${prompt.path}`"
      :busy="prompt.busy"
      size="sm"
      @close="prompt = null"
    >
      <form class="folder-form" @submit.prevent="submitPrompt">
        <input v-model="prompt.value" type="text" v-focus data-testid="folder-name" spellcheck="false" placeholder="Folder name" />
        <p v-if="prompt.kind === 'new'" class="hint">An empty folder is made on disk and stays in the tree until you remove it.</p>
        <p v-else class="hint">Every book in it moves, with its sub-folders and file names. You will see the plan first, and can undo it.</p>
        <p v-if="prompt.error" class="error" role="alert">{{ prompt.error }}</p>
        <div class="buttons">
          <button type="submit" class="primary" :disabled="prompt.busy" data-testid="folder-name-ok">{{ prompt.kind === "new" ? "Create" : "Preview" }}</button>
          <button type="button" @click="prompt = null">Cancel</button>
        </div>
      </form>
    </Dialog>

    <Dialog v-if="flowOpen && state.action" :title="planTitle()" :busy="state.phase === 'running'" size="md" data-testid="folder-flow" @close="closeFlow">
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
      <template #footer>
        <button v-if="state.phase === 'confirm'" type="button" class="primary" data-testid="folder-run" @click="runFlow">{{ state.action.kind === "move" ? "Move" : "Rename" }} {{ state.plan?.books.toLocaleString("en-US") }} book{{ state.plan?.books === 1 ? "" : "s" }}</button>
        <button v-if="state.phase === 'done'" type="button" data-testid="folder-undo" @click="flow.undo()">Undo</button>
        <button type="button" :disabled="state.phase === 'running'" @click="closeFlow">{{ state.phase === "done" || state.undone ? "Close" : "Cancel" }}</button>
      </template>
    </Dialog>
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
.folder-row:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
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
.folders-authors {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  padding: var(--sp-1) var(--sp-3);
  font-size: var(--fs-small);
  color: var(--fg-muted);
  cursor: pointer;
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
form.folder-form {
  display: flex;
  flex-direction: column;
  gap: 0.7rem;
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
