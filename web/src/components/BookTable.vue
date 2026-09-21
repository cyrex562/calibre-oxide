<script setup lang="ts">
// Sortable, resizable column table for the library (issue 1.1 of the
// #816 epic).
//
// Presentational on purpose: every decision about *what* a cell says,
// which columns exist and what a header click should do lives in
// library/columns.ts as pure, tested functions. This component does
// layout, pointer handling and events.

import { ref } from "vue";

import { formatCell, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, nextSort, type BookColumn } from "../library/columns";
import type { BookSummary } from "../library/types";

const props = defineProps<{
  books: BookSummary[];
  columns: BookColumn[];
  selectMode: boolean;
  selectedIds: Set<number>;
  selectedBookId: number | null;
  sort: string;
  sortOrder: "asc" | "desc";
}>();

const emit = defineEmits<{
  select: [bookId: number];
  toggleSelected: [bookId: number];
  sortBy: [value: { sort: string; order: "asc" | "desc" }];
  resize: [value: { key: string; width: number }];
}>();

/** The column currently driving the sort, if it is one we show. */
function sortIndicator(column: BookColumn): "" | "▲" | "▼" {
  if (!column.sortKey) return "";
  if (props.sort.split(",")[0]?.trim() !== column.sortKey) return "";
  return props.sortOrder === "asc" ? "▲" : "▼";
}

function onHeaderClick(column: BookColumn) {
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

function startResize(event: PointerEvent, column: BookColumn) {
  event.preventDefault();
  event.stopPropagation(); // never let a resize also sort the column
  resizing.value = { key: column.key, startX: event.clientX, startWidth: column.width };
  (event.target as HTMLElement).setPointerCapture(event.pointerId);
}

function onResizeMove(event: PointerEvent) {
  const state = resizing.value;
  if (!state) return;
  const width = Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, state.startWidth + (event.clientX - state.startX)));
  emit("resize", { key: state.key, width });
}

function endResize(event: PointerEvent) {
  if (!resizing.value) return;
  (event.target as HTMLElement).releasePointerCapture?.(event.pointerId);
  resizing.value = null;
}

function onRowClick(bookId: number) {
  if (props.selectMode) emit("toggleSelected", bookId);
  else emit("select", bookId);
}

/**
 * Keyboard access to a row. The table is a grid of buttons visually,
 * but rows have to stay `<tr>` for the column layout to work, so the
 * roles and key handling are supplied explicitly.
 */
function onRowKeydown(event: KeyboardEvent, bookId: number) {
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  onRowClick(bookId);
}
</script>

<template>
  <div class="table-scroll">
    <table class="book-table">
      <thead>
        <tr>
          <th v-if="selectMode" class="check-col" scope="col"></th>
          <th v-for="column in columns" :key="column.key" scope="col" :style="{ width: `${column.width}px` }" :class="{ sortable: !!column.sortKey }" :aria-sort="sortIndicator(column) === '▲' ? 'ascending' : sortIndicator(column) === '▼' ? 'descending' : 'none'">
            <button v-if="column.sortKey" type="button" class="header-button" :title="`Sort by ${column.label}`" @click="onHeaderClick(column)">
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
            ></span>
          </th>
        </tr>
      </thead>

      <tbody>
        <tr
          v-for="b in books"
          :key="b.id"
          tabindex="0"
          :class="{ selected: selectMode ? selectedIds.has(b.id) : selectedBookId === b.id }"
          @click="onRowClick(b.id)"
          @keydown="onRowKeydown($event, b.id)"
        >
          <td v-if="selectMode" class="check-col">
            <input type="checkbox" :checked="selectedIds.has(b.id)" :aria-label="`Select ${b.title}`" @click.stop="emit('toggleSelected', b.id)" />
          </td>
          <td v-for="column in columns" :key="column.key" :style="{ width: `${column.width}px` }" :class="[`kind-${column.kind}`]" :title="formatCell(b, column)">
            {{ formatCell(b, column) }}
          </td>
        </tr>
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
.table-scroll {
  overflow-x: auto;
  overflow-y: visible;
  width: 100%;
}

.book-table {
  border-collapse: collapse;
  /* `fixed` is what makes the pixel widths authoritative; with `auto`
     the browser re-derives widths from content and resizing appears
     to do nothing. */
  table-layout: fixed;
  font-size: 0.9rem;
}

.book-table th,
.book-table td {
  text-align: left;
  padding: 0.35rem 0.55rem;
  border-bottom: 1px solid #e0e0e0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.book-table thead th {
  /* Also the containing block for `.resize-handle`: `sticky` is a
     positioned element, so the handle can be absolutely positioned
     against it without an extra `relative` wrapper that would break
     the sticky behaviour. */
  position: sticky;
  top: 0;
  z-index: 1;
  background: #f5f5f5;
  border-bottom: 2px solid #d0d0d0;
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
  font-size: 0.7em;
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
  background: #bbb;
}

.book-table tbody tr {
  cursor: pointer;
}

.book-table tbody tr:hover {
  background: #f0f6ff;
}

.book-table tbody tr.selected {
  background: #dbeafe;
}

.book-table tbody tr:focus-visible {
  outline: 2px solid #2563eb;
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

@media (prefers-color-scheme: dark) {
  .book-table th,
  .book-table td {
    border-bottom-color: #333;
  }
  .book-table thead th {
    background: #222;
    border-bottom-color: #444;
  }
  .book-table tbody tr:hover {
    background: #1e293b;
  }
  .book-table tbody tr.selected {
    background: #1e3a5f;
  }
  .resize-handle:hover {
    background: #555;
  }
}
</style>
