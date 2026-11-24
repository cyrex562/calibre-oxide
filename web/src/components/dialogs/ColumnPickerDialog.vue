<script setup lang="ts">
// "Table columns": which columns the table shows and in what order. The
// preferences are the library view's; this only reports what was asked for.

import { ref } from "vue";

import type { BookColumn } from "../../library/columns";
import Dialog from "../Dialog.vue";

const props = defineProps<{
  /** Every column: the shown ones in order, then the hidden ones. */
  columns: BookColumn[];
  /** The keys of the shown columns, in order. */
  shown: string[];
}>();
const emit = defineEmits<{
  close: [];
  toggle: [key: string];
  move: [key: string, delta: number];
  /** A row was dropped on another shown row. */
  reorder: [from: string, onto: string];
  reset: [];
}>();

const dragKey = ref<string | null>(null);

function onDrop(targetKey: string) {
  const from = dragKey.value;
  dragKey.value = null;
  if (from) emit("reorder", from, targetKey);
}
</script>

<template>
  <Dialog title="Table columns" size="lg" @close="emit('close')">
    <p class="hint">Choose which columns the table shows, and their order. Drag a row, or use the arrows. You can also drag a column header in the table.</p>
    <ul class="column-list">
      <li
        v-for="column in columns"
        :key="column.key"
        class="column-row"
        :class="{ 'drag-over': dragKey !== null && dragKey !== column.key && props.shown.includes(column.key) }"
        :draggable="props.shown.includes(column.key)"
        @dragstart="dragKey = column.key"
        @dragend="dragKey = null"
        @dragover.prevent
        @drop.prevent="onDrop(column.key)"
      >
        <label class="field checkbox">
          <input type="checkbox" :checked="props.shown.includes(column.key)" @change="emit('toggle', column.key)" />
          {{ column.label }}
          <span v-if="!column.sortKey" class="hint inline">({{ column.sortNote ?? "not sortable" }})</span>
        </label>
        <!-- On every row, disabled for a hidden column, so a ticked row is no taller than an unticked one. -->
        <button type="button" :disabled="!props.shown.includes(column.key) || props.shown.indexOf(column.key) === 0" title="Move up (left in the table)" :aria-label="`Move ${column.label} up`" @click="emit('move', column.key, -1)">↑</button>
        <button type="button" :disabled="!props.shown.includes(column.key) || props.shown.indexOf(column.key) === props.shown.length - 1" title="Move down (right in the table)" :aria-label="`Move ${column.label} down`" @click="emit('move', column.key, 1)">↓</button>
      </li>
    </ul>
    <template #footer>
      <button type="button" @click="emit('reset')">Reset to default</button>
      <button type="button" class="primary" @click="emit('close')">Close</button>
    </template>
  </Dialog>
</template>

<style scoped>
.column-list {
  list-style: none;
  margin: 0 0 1rem;
  padding: 0;
  max-height: 55vh;
  overflow-y: auto;
}
.column-row {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.15rem 0;
}
.column-row .field {
  flex: 1;
}
.column-row .field.checkbox {
  flex-direction: row;
  align-items: center;
  gap: 0.4rem;
}
.column-row[draggable="true"] {
  cursor: grab;
}
.column-row.drag-over:hover {
  box-shadow: inset 0 2px 0 var(--accent);
}
.hint.inline {
  font-size: var(--fs-small);
  opacity: 0.7;
}
</style>
