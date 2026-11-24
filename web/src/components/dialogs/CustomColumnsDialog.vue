<script setup lang="ts">
// Custom columns: list, quick-add presets, make your own, delete. Real, new
// routes (see crates/lexmechanic_srv/src/custom_columns.rs's own doc for why no
// upstream route exists to port here). `is_multiple` is left out of the create
// form entirely: it is not a real working combination for any datatype this
// backend supports yet (text/comments/series reject it outright, and
// bool/int/float/rating silently ignore it -- see that module's own doc) -- a
// real, disclosed narrowing rather than a checkbox that would look like it
// works but doesn't.

import { onMounted, ref } from "vue";

import { addCustomColumn, fetchCustomColumns, removeCustomColumn } from "../../library/api";
import { COLUMN_EXPLANATIONS, COLUMN_PRESETS, columnDraftProblem, createColumn, presetDraft, type ColumnPreset } from "../../library/columnPresets";
import type { CustomColumnInfo } from "../../library/types";
import Dialog from "../Dialog.vue";
import { confirmDialog } from "./confirm";

const emit = defineEmits<{
  close: [];
  /** A column was added or removed: the picker, the category browser and the details follow without a reload. */
  changed: [];
}>();

const columnsError = ref<string | null>(null);
const customColumns = ref<Record<string, CustomColumnInfo>>({});
const newColumnLabel = ref("");
const newColumnName = ref("");
const newColumnDatatype = ref("text");

async function loadCustomColumns() {
  try {
    customColumns.value = await fetchCustomColumns();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}
onMounted(() => void loadCustomColumns());

// One-click presets (edition, game line, volume): pick one, edit the name, create.
const presetPicked = ref<{ preset: ColumnPreset; label: string; name: string; datatype: string } | null>(null);
function pickColumnPreset(preset: ColumnPreset) {
  presetPicked.value = { preset, ...presetDraft(preset) };
  columnsError.value = null;
}
async function createPresetColumn() {
  const d = presetPicked.value;
  if (!d) return;
  const problem = columnDraftProblem(d, Object.keys(customColumns.value));
  if (problem) {
    columnsError.value = problem;
    return;
  }
  columnsError.value = null;
  try {
    await createColumn(d);
    presetPicked.value = null;
    await onColumnsChanged();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}
async function onColumnsChanged() {
  await loadCustomColumns();
  emit("changed");
}

async function createCustomColumn() {
  if (!newColumnLabel.value.trim() || !newColumnName.value.trim()) return;
  columnsError.value = null;
  try {
    await addCustomColumn(newColumnLabel.value.trim(), newColumnName.value.trim(), newColumnDatatype.value);
    newColumnLabel.value = "";
    newColumnName.value = "";
    await onColumnsChanged();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeCustomColumnClick(label: string) {
  if (!(await confirmDialog(`Delete the custom column "${label}"? Its stored values will be lost.`, { confirmLabel: "Delete", danger: true }))) return;
  columnsError.value = null;
  try {
    await removeCustomColumn(label);
    await onColumnsChanged();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <Dialog title="Custom columns" show-close size="lg" @close="emit('close')">
    <section>
      <p v-if="columnsError" class="error">{{ columnsError }}</p>
      <ul class="manage-list">
        <li v-for="[label, col] in Object.entries(customColumns)" :key="label">
          <span class="manage-name">{{ col.name }}</span>
          <code class="manage-query">#{{ label }} ({{ col.datatype }})</code>
          <button type="button" class="manage-remove" @click="removeCustomColumnClick(label)">Delete</button>
        </li>
        <li v-if="Object.keys(customColumns).length === 0">No custom columns yet.</li>
      </ul>
      <div class="column-presets" data-testid="column-presets">
        <h4>Quick add</h4>
        <div class="preset-buttons">
          <button v-for="p in COLUMN_PRESETS" :key="p.id" type="button" :data-testid="`preset-${p.id}`" :disabled="p.label in customColumns" :title="p.label in customColumns ? `#${p.label} already exists` : p.hint" @click="pickColumnPreset(p)">{{ p.title }}</button>
        </div>
        <form v-if="presetPicked" class="manage-form" data-testid="preset-form" @submit.prevent="createPresetColumn">
          <input v-model="presetPicked.name" aria-label="Display name" data-testid="preset-name" required />
          <code class="manage-query">#{{ presetPicked.label }} ({{ presetPicked.datatype }})</code>
          <button type="submit" data-testid="preset-create">Create</button>
        </form>
        <p v-if="presetPicked" class="hint">Creates the column <code>#{{ presetPicked.label }}</code> as {{ presetPicked.datatype === "int" ? "a whole number" : "text" }}. {{ presetPicked.preset.hint }}</p>
        <ul class="hint column-notes">
          <li v-for="line in COLUMN_EXPLANATIONS" :key="line">{{ line }}</li>
        </ul>
      </div>
      <h4>Or make your own</h4>
      <form class="manage-form" @submit.prevent="createCustomColumn">
        <input v-model="newColumnLabel" placeholder="Label (e.g. shelf)" pattern="[a-zA-Z0-9_]+" title="Letters, numbers, and underscores only" required />
        <input v-model="newColumnName" placeholder="Display name (e.g. Shelf)" required />
        <select v-model="newColumnDatatype">
          <option value="text">Text</option>
          <option value="comments">Long text</option>
          <option value="int">Integer</option>
          <option value="float">Decimal number</option>
          <option value="bool">Yes/No</option>
          <option value="rating">Rating</option>
        </select>
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
.preset-buttons {
  display: flex;
  gap: 0.5rem;
  flex-wrap: wrap;
  margin-bottom: 0.4rem;
}
.column-notes {
  margin: 0.4rem 0;
  padding-left: 1.1rem;
}
</style>
