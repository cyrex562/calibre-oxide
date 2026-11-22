<script setup lang="ts">
// Sortable, resizable, virtualised column table for the library.
//
// One continuous list: only the rows near the viewport are in the DOM,
// between two spacer rows that stand in for the rest, so the scrollbar
// represents every matching book however many there are.
//
// Presentational on purpose: every decision about *what* a cell says,
// which columns exist and what a header click should do lives in
// library/columns.ts; what an edit means lives in library/inlineEdit.ts;
// which rows exist and which are loaded lives in library/bookWindow.ts.
// This component does layout, pointer handling and events.

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { autoFitWidth, CHECK_COLUMN_WIDTH, dropIndex, isDrag, moveKeyTo, resizedColumnWidth, tableWidth } from "../library/columnLayout";
import { formatCell, nextSort, type BookColumn } from "../library/columns";
import { defaultEditKey, editKeyAction, initialText, isEditable, isEmptyCell, neighbourEditableKey, startsEditing, type CommitResult } from "../library/inlineEdit";
import type { BookSummary } from "../library/types";
import { windowRange } from "../library/virtualList";
import { dragIds, setBooksDrag } from "../library/folders";

/** Height of one row, in pixels. Fixed: virtual scrolling depends on it. */
const ROW_HEIGHT = 30;
/** Used until the scroller has been measured (and in jsdom, which never measures). */
const FALLBACK_VIEWPORT = 600;

const props = defineProps<{
  /** Every row's book id, in order. */
  ids: number[];
  /** The loaded summary for an id, or undefined while it is still loading. */
  getBook: (id: number) => BookSummary | undefined;
  columns: BookColumn[];
  selectMode: boolean;
  selectedIds: Set<number>;
  selectedBookId: number | null;
  sort: string;
  sortOrder: "asc" | "desc";
  /**
   * Row colours from the colouring rules (#4.1), keyed by book id.
   * Already validated against an allowlist -- see
   * library/coloringRules.ts for why that matters before a value
   * reaches a style attribute.
   */
  rowColors?: Record<number, string>;
  /** Changes when the list is for a new query: scrolls back to the top. */
  resetKey?: number;
  /** Saves an in-place edit. Without it cells are read-only. */
  commit?: (bookId: number, column: BookColumn, raw: string) => Promise<CommitResult>;
}>();

const emit = defineEmits<{
  select: [bookId: number];
  toggleSelected: [bookId: number];
  rangeSelect: [bookId: number];
  sortBy: [value: { sort: string; order: "asc" | "desc" }];
  resize: [value: { key: string; width: number }];
  /** A header was dragged: move column `key` before column slot `index` (pre-move index). */
  reorder: [value: { key: string; index: number }];
  contextMenu: [value: { bookId: number; x: number; y: number }];
  /** Rows `[first, last)` are rendered (visible plus overscan): load them. */
  range: [value: { first: number; last: number }];
  /** A navigation key (arrows, page keys, Home/End) on the list. */
  navigate: [key: string];
}>();

// ---------------------------------------------------------------
// Windowing
// ---------------------------------------------------------------

const scroller = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewportHeight = ref(FALLBACK_VIEWPORT);
const headerHeight = ref(0);

const range = computed(() => windowRange({ count: props.ids.length, rowHeight: ROW_HEIGHT, scrollTop: scrollTop.value, viewportHeight: Math.max(ROW_HEIGHT, viewportHeight.value - headerHeight.value) }));
const rowIndexes = computed(() => {
  const out: number[] = [];
  for (let i = range.value.start; i < range.value.end; i++) out.push(i);
  return out;
});

watch(
  () => [range.value.start, range.value.end] as const,
  ([first, last]) => emit("range", { first, last }),
  { immediate: true },
);

function measure() {
  const el = scroller.value;
  if (!el) return;
  if (el.clientHeight > 0) viewportHeight.value = el.clientHeight;
  headerHeight.value = el.querySelector("thead")?.getBoundingClientRect().height ?? 0;
}

function onScroll() {
  const el = scroller.value;
  if (el) scrollTop.value = el.scrollTop;
}

let resizeObserver: ResizeObserver | null = null;
onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && scroller.value) {
    resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(scroller.value);
  }
});
onBeforeUnmount(() => resizeObserver?.disconnect());

watch(
  () => props.resetKey,
  () => {
    if (scroller.value) scroller.value.scrollTop = 0;
    scrollTop.value = 0;
    editing.value = null;
  },
);

/** Scrolls so row `index` is fully visible, below the sticky header. */
function scrollToIndex(index: number) {
  const el = scroller.value;
  if (!el) return;
  const viewport = Math.max(ROW_HEIGHT, (el.clientHeight || viewportHeight.value) - headerHeight.value);
  const top = index * ROW_HEIGHT;
  let next = el.scrollTop;
  if (top < next) next = top;
  else if (top + ROW_HEIGHT > next + viewport) next = top + ROW_HEIGHT - viewport;
  if (next !== el.scrollTop) el.scrollTop = next;
  scrollTop.value = el.scrollTop;
}

/** How many rows fit on screen, for PageUp/PageDown. */
function pageRows(): number {
  return Math.max(1, Math.floor((viewportHeight.value - headerHeight.value) / ROW_HEIGHT));
}

/** The scroll position's visible rows, for the status bar. */
function visibleRows() {
  return { first: range.value.first, last: range.value.last };
}

defineExpose({ scrollToIndex, pageRows, visibleRows, startEdit: (id: number, key?: string) => startEdit(id, key) });

/** The column currently driving the sort, if it is one we show. */
function sortIndicator(column: BookColumn): "" | "▲" | "▼" {
  if (!column.sortKey) return "";
  if (props.sort.split(",")[0]?.trim() !== column.sortKey) return "";
  return props.sortOrder === "asc" ? "▲" : "▼";
}

function onHeaderClick(column: BookColumn) {
  if (suppressClick) return;
  const next = nextSort(column, props.sort, props.sortOrder);
  if (next) emit("sortBy", next);
}

// ---------------------------------------------------------------
// Column resizing
// ---------------------------------------------------------------
//
// Pointer events rather than mouse events, so a trackpad or touch
// drag works identically, and `setPointerCapture` so the drag keeps
// tracking when the pointer leaves the 6px handle -- without it, any
// drag faster than the render loop silently stops.

const resizing = ref<{ key: string; startX: number; startWidth: number } | null>(null);

/** The table is exactly as wide as its columns (see library/columnLayout.ts). */
const totalWidth = computed(() => tableWidth(props.columns, props.selectMode));

function startResize(event: PointerEvent, column: BookColumn) {
  event.preventDefault();
  event.stopPropagation(); // never let a resize also sort or drag the column
  resizing.value = { key: column.key, startX: event.clientX, startWidth: column.width };
  (event.target as HTMLElement).setPointerCapture?.(event.pointerId);
}

function onResizeMove(event: PointerEvent) {
  const state = resizing.value;
  if (!state) return;
  emit("resize", { key: state.key, width: resizedColumnWidth(state.startWidth, event.clientX - state.startX) });
}

function endResize(event: PointerEvent) {
  if (!resizing.value) return;
  (event.target as HTMLElement).releasePointerCapture?.(event.pointerId);
  resizing.value = null;
}

let measureCanvas: HTMLCanvasElement | null = null;

/** Pixel width of `text` in the table's own font. */
function measureText(text: string, font: string): number {
  try {
    measureCanvas ??= document.createElement("canvas");
    const ctx = measureCanvas.getContext("2d");
    if (ctx) {
      ctx.font = font;
      return ctx.measureText(text).width;
    }
  } catch {
    // jsdom has no canvas
  }
  return text.length * 7;
}

/** Double-click on a handle: fit the column to its header and the loaded rows. */
function autoFit(column: BookColumn) {
  const el = scroller.value;
  const sample = el?.querySelector("tbody td") ?? el?.querySelector("thead th");
  const style = sample ? getComputedStyle(sample) : null;
  const font = style ? `${style.fontWeight} ${style.fontSize} ${style.fontFamily}` : "13px sans-serif";
  const cells: string[] = [];
  for (const id of props.ids) {
    const book = props.getBook(id);
    if (book) cells.push(formatCell(book, column));
  }
  emit("resize", { key: column.key, width: autoFitWidth(column.label, cells, (t) => measureText(t, font)) });
}

// ---------------------------------------------------------------
// Column reordering
// ---------------------------------------------------------------
//
// A press on a header is a click (sort) until the pointer has moved a
// few pixels; only then does it become a drag. Pointer capture is taken
// only once it is a drag -- capturing at pointerdown would retarget the
// eventual click away from the header's button and break sorting.

const headerDrag = ref<{ key: string; startX: number; startY: number; active: boolean; index: number } | null>(null);
/** Swallows the click the browser fires after a drag ends on a header. */
let suppressClick = false;

function headerSpans(): { left: number; right: number }[] {
  const ths = scroller.value?.querySelectorAll<HTMLElement>("thead th[data-key]") ?? [];
  return Array.from(ths).map((th) => {
    const r = th.getBoundingClientRect();
    return { left: r.left, right: r.right };
  });
}

function onHeaderPointerDown(event: PointerEvent, column: BookColumn) {
  if (event.button !== 0) return;
  headerDrag.value = { key: column.key, startX: event.clientX, startY: event.clientY, active: false, index: -1 };
}

function onHeaderPointerMove(event: PointerEvent) {
  const state = headerDrag.value;
  if (!state) return;
  if (!state.active) {
    if (!isDrag(event.clientX - state.startX, event.clientY - state.startY)) return;
    state.active = true;
    (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
  }
  state.index = dropIndex(headerSpans(), event.clientX);
}

function onHeaderPointerUp(event: PointerEvent) {
  const state = headerDrag.value;
  headerDrag.value = null;
  if (!state) return;
  (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
  if (!state.active) return;
  suppressClick = true;
  setTimeout(() => (suppressClick = false), 0);
  const keys = props.columns.map((c) => c.key);
  if (state.index >= 0 && moveKeyTo(keys, state.key, state.index) !== keys) emit("reorder", { key: state.key, index: state.index });
}

function onHeaderPointerCancel() {
  headerDrag.value = null;
}

/** The slot a drop would land in, as a column index, while dragging. */
const dropAt = computed(() => (headerDrag.value?.active ? headerDrag.value.index : -1));

/** The header edge to draw the drop indicator on. */
function dropClass(index: number): string {
  if (dropAt.value < 0) return "";
  if (index === dropAt.value) return "drop-before";
  if (index === props.columns.length - 1 && dropAt.value === props.columns.length) return "drop-after";
  return "";
}

// ---------------------------------------------------------------
// Rows
// ---------------------------------------------------------------

/** Dragging a row to a folder in the category browser carries the selection (or the row alone). */
function onRowDragStart(bookId: number, event: DragEvent) {
  if (event.dataTransfer) setBooksDrag(event.dataTransfer, dragIds(bookId, props.selectedIds, props.selectMode));
}

function onRowClick(bookId: number, event?: MouseEvent) {
  if (props.selectMode) {
    if (event?.shiftKey) emit("rangeSelect", bookId);
    else emit("toggleSelected", bookId);
  } else emit("select", bookId);
}

/**
 * Keyboard access to a row. The table is a grid of buttons visually,
 * but rows have to stay `<tr>` for the column layout to work, so the
 * roles and key handling are supplied explicitly.
 */
function onRowKeydown(event: KeyboardEvent, bookId: number) {
  if (editing.value) return;
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  onRowClick(bookId);
}

const NAV_KEYS = new Set(["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End"]);

function onListKeydown(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null;
  if (target && (target.tagName === "INPUT" || target.tagName === "SELECT" || target.tagName === "TEXTAREA")) return;
  if (event.ctrlKey || event.metaKey || event.altKey) return;
  if (NAV_KEYS.has(event.key)) {
    event.preventDefault();
    emit("navigate", event.key);
    scroller.value?.focus({ preventScroll: true });
    return;
  }
  if (startsEditing(event) && props.commit) {
    const id = props.selectedBookId ?? (target?.closest("tr")?.getAttribute("data-id") ? Number(target?.closest("tr")?.getAttribute("data-id")) : null);
    if (id === null) return;
    event.preventDefault();
    startEdit(id, lastCellKey.value ?? undefined);
  }
}

// ---------------------------------------------------------------
// In-place editing
// ---------------------------------------------------------------

interface EditState {
  id: number;
  key: string;
  text: string;
  error: string | null;
  busy: boolean;
}

const editing = ref<EditState | null>(null);
/** The column of the cell last clicked: F2 edits that one. */
const lastCellKey = ref<string | null>(null);
/** Set while an editor is being closed on purpose, so its blur does not also save. */
let closing = false;

function columnByKey(key: string): BookColumn | undefined {
  return props.columns.find((c) => c.key === key);
}

function startEdit(id: number, key?: string) {
  if (!props.commit) return;
  const book = props.getBook(id);
  const column = columnByKey(key ?? "") && isEditable(columnByKey(key ?? "")!) ? columnByKey(key!)! : columnByKey(defaultEditKey(props.columns) ?? "");
  if (!book || !column || !isEditable(column)) return;
  closing = false;
  editing.value = { id, key: column.key, text: initialText(book, column), error: null, busy: false };
}

function onCellClick(event: MouseEvent, book: BookSummary, column: BookColumn) {
  lastCellKey.value = column.key;
  if (!props.commit || props.selectMode || !isEditable(column)) return;
  if (event.shiftKey || event.ctrlKey || event.metaKey) return;
  if (editing.value && editing.value.id === book.id && editing.value.key === column.key) return;
  if (isEmptyCell(book, column)) startEdit(book.id, column.key);
}

function onCellDblClick(book: BookSummary, column: BookColumn) {
  if (!props.commit || !isEditable(column)) return;
  startEdit(book.id, column.key);
}

async function finishEdit(then: "stay" | "next" | "prev" = "stay") {
  const state = editing.value;
  if (!state || state.busy || !props.commit) return;
  const column = columnByKey(state.key);
  if (!column) {
    editing.value = null;
    return;
  }
  closing = true;
  state.busy = true;
  let result: CommitResult;
  try {
    result = await props.commit(state.id, column, state.text);
  } catch (e) {
    result = { status: "error", message: e instanceof Error ? e.message : String(e) };
  }
  state.busy = false;
  if (editing.value !== state) return; // the list was reset while saving
  if (result.status === "error") {
    // Keep the editor open with what was typed.
    state.error = result.message;
    closing = false;
    await nextTick();
    focusEditor();
    return;
  }
  editing.value = null;
  if (then !== "stay") {
    const nextKey = neighbourEditableKey(props.columns, state.key, then === "next" ? 1 : -1);
    if (nextKey) startEdit(state.id, nextKey);
  }
}

function cancelEdit() {
  closing = true;
  editing.value = null;
  void nextTick(() => scroller.value?.focus({ preventScroll: true }));
}

function onEditorKeydown(event: KeyboardEvent) {
  const action = editKeyAction(event);
  if (!action) return;
  event.preventDefault();
  event.stopPropagation();
  if (action === "cancel") cancelEdit();
  else if (action === "save") void finishEdit("stay").then(() => scroller.value?.focus({ preventScroll: true }));
  else void finishEdit(action);
}

function onEditorBlur() {
  if (closing) return;
  void finishEdit("stay");
}

function focusEditor() {
  scroller.value?.querySelector<HTMLElement>(".cell-editor")?.focus();
}

const vFocus = {
  mounted(el: HTMLElement) {
    el.focus();
    if (el instanceof HTMLInputElement && el.type === "text") el.select();
  },
};

function isEditingCell(id: number, column: BookColumn): boolean {
  return editing.value !== null && editing.value.id === id && editing.value.key === column.key;
}

const BOOL_OPTIONS = [
  { value: "", label: "" },
  { value: "true", label: "Yes" },
  { value: "false", label: "No" },
];

function hint(column: BookColumn): string {
  return `Set ${column.label.toLowerCase()}…`;
}
</script>

<template>
  <div ref="scroller" class="table-scroll" tabindex="-1" @scroll.passive="onScroll" @keydown="onListKeydown">
    <table class="book-table" :style="{ width: `${totalWidth}px`, minWidth: `${totalWidth}px` }">
      <colgroup>
        <col v-if="selectMode" :style="{ width: `${CHECK_COLUMN_WIDTH}px` }" />
        <col v-for="column in columns" :key="column.key" :style="{ width: `${column.width}px` }" />
      </colgroup>
      <thead>
        <tr>
          <th v-if="selectMode" class="check-col" scope="col"></th>
          <th
            v-for="(column, ci) in columns"
            :key="column.key"
            scope="col"
            :data-key="column.key"
            :style="{ width: `${column.width}px` }"
            :class="[{ sortable: !!column.sortKey, dragging: headerDrag?.active && headerDrag.key === column.key }, dropClass(ci)]"
            :title="column.tooltip"
            :aria-sort="sortIndicator(column) === '▲' ? 'ascending' : sortIndicator(column) === '▼' ? 'descending' : 'none'"
            @pointerdown="onHeaderPointerDown($event, column)"
            @pointermove="onHeaderPointerMove"
            @pointerup="onHeaderPointerUp"
            @pointercancel="onHeaderPointerCancel"
          >
            <button v-if="column.sortKey" type="button" class="header-button" :title="column.tooltip ? `${column.tooltip}. Click to sort` : `Sort by ${column.label}`" @click="onHeaderClick(column)">
              <span class="header-label">{{ column.label }}</span>
              <span class="sort-indicator">{{ sortIndicator(column) }}</span>
            </button>
            <span v-else class="header-label plain">{{ column.label }}</span>

            <span
              class="resize-handle"
              role="separator"
              :aria-label="`Resize ${column.label}`"
              @pointerdown="startResize($event, column)"
              @pointermove="onResizeMove"
              @pointerup="endResize"
              @pointercancel="endResize"
              @dblclick.stop="autoFit(column)"
              @click.stop
            ></span>
          </th>
        </tr>
      </thead>

      <tbody>
        <tr v-if="range.padTop > 0" class="spacer" aria-hidden="true" :style="{ height: `${range.padTop}px` }"><td :colspan="columns.length + (selectMode ? 1 : 0)"></td></tr>
        <template v-for="i in rowIndexes" :key="ids[i]">
          <tr
            v-if="getBook(ids[i])"
            :data-id="ids[i]"
            tabindex="0"
            draggable="true"
            @dragstart="onRowDragStart(ids[i], $event)"
            :class="{ selected: selectMode ? selectedIds.has(ids[i]) : selectedBookId === ids[i] }"
            :style="rowColors?.[ids[i]] ? { color: rowColors[ids[i]] } : undefined"
            @click="onRowClick(ids[i], $event)"
            @keydown="onRowKeydown($event, ids[i])"
            @contextmenu.prevent="emit('contextMenu', { bookId: ids[i], x: $event.clientX, y: $event.clientY })"
          >
            <td v-if="selectMode" class="check-col">
              <input type="checkbox" :checked="selectedIds.has(ids[i])" :aria-label="`Select ${getBook(ids[i])!.title}`" @click.stop="onRowClick(ids[i], $event)" />
            </td>
            <td
              v-for="column in columns"
              :key="column.key"
              :style="{ width: `${column.width}px` }"
              :class="[`kind-${column.kind}`, { editable: commit && isEditable(column), 'empty-editable': commit && isEditable(column) && isEmptyCell(getBook(ids[i])!, column), editing: isEditingCell(ids[i], column), 'has-error': isEditingCell(ids[i], column) && editing?.error }]"
              :data-hint="commit && isEditable(column) ? hint(column) : undefined"
              :title="isEditingCell(ids[i], column) && editing?.error ? editing.error : formatCell(getBook(ids[i])!, column)"
              @click="onCellClick($event, getBook(ids[i])!, column)"
              @dblclick="onCellDblClick(getBook(ids[i])!, column)"
            >
              <template v-if="isEditingCell(ids[i], column) && editing">
                <select v-if="column.editor?.type === 'enum' || column.editor?.type === 'bool'" v-model="editing.text" v-focus class="cell-editor" :disabled="editing.busy" @keydown="onEditorKeydown" @blur="onEditorBlur" @click.stop @dblclick.stop @change="finishEdit('stay')">
                  <template v-if="column.editor?.type === 'enum'">
                    <option value=""></option>
                    <option v-for="v in column.editor.values" :key="v" :value="v">{{ v }}</option>
                  </template>
                  <template v-else>
                    <option v-for="o in BOOL_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</option>
                  </template>
                </select>
                <input v-else v-model="editing.text" v-focus class="cell-editor" :type="column.editor?.type === 'date' ? 'date' : 'text'" :placeholder="column.editor?.type === 'rating' ? '0 to 5' : ''" :disabled="editing.busy" :aria-label="`Edit ${column.label}`" @keydown="onEditorKeydown" @blur="onEditorBlur" @click.stop @dblclick.stop />
                <span v-if="editing.error" class="cell-error" role="alert">{{ editing.error }}</span>
              </template>
              <template v-else>{{ formatCell(getBook(ids[i])!, column) }}</template>
            </td>
          </tr>
          <tr v-else class="placeholder" :data-id="ids[i]" aria-hidden="true">
            <td v-if="selectMode" class="check-col"></td>
            <td v-for="column in columns" :key="column.key" :style="{ width: `${column.width}px` }"><span class="ph"></span></td>
          </tr>
        </template>
        <tr v-if="range.padBottom > 0" class="spacer" aria-hidden="true" :style="{ height: `${range.padBottom}px` }"><td :colspan="columns.length + (selectMode ? 1 : 0)"></td></tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
/*
  The table owns the horizontal scrolling. Without this the page body
  scrolls sideways once columns are wider than the viewport, which
  drags the toolbar and sidebar off-screen with it.
*/
.table-scroll:focus {
  outline: none;
}
.table-scroll {
  /* Fills the column and scrolls inside itself, both axes.
     Without `flex: 1` the table sizes to its own content, so an empty
     or short library left the list hugging the top of the window with
     a screen of dead space under it, and the pagination stranded in
     the middle. `min-height: 0` is what actually permits the shrink --
     a flex item defaults to `min-height: auto` and refuses to go below
     its content, which silently defeats the overflow. */
  flex: 1;
  min-height: 0;
  overflow: auto;
  width: 100%;
}

.book-table {
  border-collapse: collapse;
  /* `fixed` is what makes the pixel widths authoritative; with `auto`
     the browser re-derives widths from content and resizing appears
     to do nothing. */
  table-layout: fixed;
  /* The width is set inline to the sum of the column widths. Fixed
     layout ignores column widths while the table's own width is auto,
     which is what made resizing appear broken. */
  max-width: none;
  font-size: var(--fs-body);
}

.book-table th,
.book-table td {
  text-align: left;
  padding: 0.35rem 0.55rem;
  border-bottom: 1px solid var(--border);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Rows are a fixed 30px (ROW_HEIGHT in the script): the virtual list
   computes positions from it, so a row must never grow. */
.book-table tbody td {
  box-sizing: border-box;
  height: 30px;
  padding-top: 0;
  padding-bottom: 0;
  line-height: 29px;
}
.book-table tbody tr.spacer td {
  height: inherit;
  padding: 0;
  border: 0;
}
.book-table tbody tr.spacer {
  pointer-events: none;
}
.book-table tbody tr.placeholder td {
  opacity: 0.5;
}
.ph {
  display: inline-block;
  width: 60%;
  height: 0.7em;
  border-radius: 3px;
  background: var(--border);
  vertical-align: middle;
}

/* Editing in place. */
.book-table td.editable.empty-editable {
  cursor: text;
}
.book-table td.empty-editable:not(.editing):hover::after {
  content: attr(data-hint);
  color: var(--fg-faint);
  font-style: italic;
}
.book-table td.editing {
  padding: 0 2px;
  overflow: visible;
  position: relative;
}
.cell-editor {
  box-sizing: border-box;
  width: 100%;
  height: 26px;
  font: inherit;
  padding: 0 0.4rem;
}
.book-table td.has-error .cell-editor {
  outline: 2px solid var(--danger);
}
.cell-error {
  position: absolute;
  left: 2px;
  top: 100%;
  z-index: 3;
  background: var(--danger);
  color: #fff;
  font-size: var(--fs-small);
  line-height: 1.3;
  padding: 0.15rem 0.45rem;
  border-radius: 3px;
  white-space: normal;
  max-width: 320px;
}

.book-table thead th {
  /* Also the containing block for `.resize-handle`: `sticky` is a
     positioned element, so the handle can be absolutely positioned
     against it without an extra `relative` wrapper that would break
     the sticky behaviour. */
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--bg-bar);
  border-bottom: 2px solid var(--border);
  font-weight: 600;
  user-select: none;
}

.header-button {
  all: unset;
  cursor: pointer;
  display: inline-flex;
  align-items: baseline;
  gap: 0.3rem;
  width: calc(100% - 8px);
  overflow: hidden;
}

.header-label {
  overflow: hidden;
  text-overflow: ellipsis;
}

.sort-indicator {
  font-size: var(--fs-micro);
  opacity: 0.75;
}

.resize-handle {
  position: absolute;
  top: 0;
  right: 0;
  width: 6px;
  height: 100%;
  cursor: col-resize;
  /* A hit target this narrow is hard to grab without a hint. */
  touch-action: none;
}

.resize-handle:hover {
  background: linear-gradient(to right, transparent 2px, var(--accent) 2px, var(--accent) 4px, transparent 4px);
}
/* A faint divider is always visible so the handle can be found. */
.resize-handle::before {
  content: "";
  position: absolute;
  right: 2px;
  top: 25%;
  height: 50%;
  width: 1px;
  background: var(--border);
}

/* Column reordering: the dragged header dims; the drop slot gets a bar. */
.book-table thead th {
  cursor: grab;
}
.book-table thead th.dragging {
  opacity: 0.5;
}
.book-table thead th.drop-before {
  box-shadow: inset 3px 0 0 var(--accent);
}
.book-table thead th.drop-after {
  box-shadow: inset -3px 0 0 var(--accent);
}

.book-table tbody tr {
  cursor: pointer;
}

.book-table tbody tr:hover {
  background: var(--accent-soft);
}

.book-table tbody tr.selected {
  background: var(--bg-selected);
}

.book-table tbody tr:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}

.check-col {
  width: 34px;
  min-width: 34px;
}

.kind-number,
.kind-rating {
  font-variant-numeric: tabular-nums;
}

</style>
