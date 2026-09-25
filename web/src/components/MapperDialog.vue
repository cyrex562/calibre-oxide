<script setup lang="ts">
// Author / tag mapping rules (issue 1.7 of the #816 epic).
//
// Rules are edited here, but nothing is written until the user has
// seen a preview: a mapping rule applied across a library is not
// undoable, since nothing records what an author was called before
// the rule rewrote it. The Apply button stays disabled until a
// preview has run, so "apply" always means "apply what I just read".

import { computed, ref } from "vue";

import { applyMapper, previewMapper, type MappedBook, type MapperRule } from "../library/api";

const props = defineProps<{
  /** Books to act on. Empty means the whole library. */
  bookIds: number[];
}>();

const emit = defineEmits<{ close: []; applied: [] }>();

const MATCH_TYPES = [
  { value: "one_of", label: "is one of" },
  { value: "not_one_of", label: "is not one of" },
  { value: "has", label: "contains" },
  { value: "matches", label: "matches regex" },
  { value: "not_matches", label: "does not match regex" },
];

const field = ref<"authors" | "tags">("authors");

// The author engine only replaces; the tag engine can also keep or
// remove. Offering an action the chosen field cannot perform would
// just produce a 400 from the server's own validation.
const actions = computed(() => (field.value === "authors" ? [{ value: "replace", label: "Replace with" }] : [
  { value: "replace", label: "Replace with" },
  { value: "remove", label: "Remove" },
  { value: "keep", label: "Keep" },
]));

function blankRule(): MapperRule {
  return { action: "replace", query: "", match_type: "one_of", replace: "" };
}

const rules = ref<MapperRule[]>([blankRule()]);
const preview = ref<MappedBook[] | null>(null);
const busy = ref(false);
const error = ref<string | null>(null);
const summary = ref<string | null>(null);

/** Switching field can strand an action the new field cannot perform. */
function onFieldChange() {
  const allowed = new Set(actions.value.map((a) => a.value));
  for (const rule of rules.value) {
    if (!allowed.has(rule.action)) rule.action = "replace";
  }
  preview.value = null;
}

function addRule() {
  rules.value.push(blankRule());
  preview.value = null;
}

function removeRule(index: number) {
  rules.value.splice(index, 1);
  if (rules.value.length === 0) rules.value.push(blankRule());
  preview.value = null;
}

const usableRules = computed(() => rules.value.filter((r) => r.query.trim() !== ""));

async function runPreview() {
  if (usableRules.value.length === 0) {
    error.value = "Add at least one rule with something to match on.";
    return;
  }
  busy.value = true;
  error.value = null;
  summary.value = null;
  try {
    const result = await previewMapper(field.value, usableRules.value, props.bookIds);
    preview.value = result.books;
    if (result.changed === 0) summary.value = "No books would change.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
    preview.value = null;
  } finally {
    busy.value = false;
  }
}

async function runApply() {
  if (!preview.value || preview.value.length === 0) return;
  busy.value = true;
  error.value = null;
  try {
    const result = await applyMapper(field.value, usableRules.value, props.bookIds);
    summary.value = `Updated ${result.changed} book(s).`;
    preview.value = null;
    emit("applied");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}

const scopeLabel = computed(() => (props.bookIds.length > 0 ? `${props.bookIds.length} selected book(s)` : "the whole library"));
</script>

<template>
  <div class="manage-backdrop" @click.self="emit('close')">
    <div class="manage-panel mapper-panel">
      <h3>Map {{ field }}</h3>
      <p class="hint">Rewrite {{ field }} in bulk across {{ scopeLabel }}. Nothing is written until you preview.</p>

      <label class="field">
        Field
        <select v-model="field" :disabled="busy" @change="onFieldChange">
          <option value="authors">Authors</option>
          <option value="tags">Tags</option>
        </select>
      </label>

      <ul class="rule-list">
        <li v-for="(rule, i) in rules" :key="i" class="rule-row">
          <select v-model="rule.match_type" :disabled="busy" @change="preview = null">
            <option v-for="m in MATCH_TYPES" :key="m.value" :value="m.value">{{ m.label }}</option>
          </select>
          <input v-model="rule.query" placeholder="value or pattern…" :disabled="busy" @input="preview = null" />
          <select v-model="rule.action" :disabled="busy" @change="preview = null">
            <option v-for="a in actions" :key="a.value" :value="a.value">{{ a.label }}</option>
          </select>
          <input v-if="rule.action === 'replace'" v-model="rule.replace" placeholder="replacement…" :disabled="busy" @input="preview = null" />
          <button type="button" :disabled="busy" title="Remove this rule" @click="removeRule(i)">✕</button>
        </li>
      </ul>
      <button type="button" :disabled="busy" @click="addRule">+ Add rule</button>

      <div class="mapper-actions">
        <button type="button" :disabled="busy" @click="runPreview">{{ busy ? "Working…" : "Preview" }}</button>
        <button type="button" :disabled="busy || !preview || preview.length === 0" :title="preview ? '' : 'Preview first'" @click="runApply">
          Apply{{ preview && preview.length ? ` to ${preview.length} book(s)` : "" }}
        </button>
        <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      </div>

      <p v-if="error" class="error">{{ error }}</p>
      <p v-if="summary" class="status">{{ summary }}</p>

      <div v-if="preview && preview.length" class="preview">
        <p class="hint">{{ preview.length }} book(s) would change:</p>
        <ul class="preview-list">
          <li v-for="b in preview" :key="b.book_id">
            <span class="preview-title">{{ b.title }}</span>
            <span class="preview-before">{{ b.before.join(field === "authors" ? " & " : ", ") }}</span>
            <span class="preview-arrow">→</span>
            <span class="preview-after">{{ b.after.join(field === "authors" ? " & " : ", ") }}</span>
          </li>
        </ul>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mapper-panel {
  min-width: min(760px, 92vw);
}
.rule-list {
  list-style: none;
  margin: 0.5rem 0;
  padding: 0;
}
.rule-row {
  display: flex;
  gap: 0.35rem;
  align-items: center;
  margin-bottom: 0.35rem;
  flex-wrap: wrap;
}
.rule-row input {
  flex: 1;
  min-width: 10ch;
}
.mapper-actions {
  display: flex;
  gap: 0.5rem;
  margin: 0.75rem 0 0.5rem;
}
.preview {
  max-height: 40vh;
  overflow-y: auto;
  border-top: 1px solid var(--border);
  padding-top: 0.5rem;
}
.preview-list {
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: var(--fs-small);
}
.preview-list li {
  display: grid;
  grid-template-columns: minmax(8ch, 1fr) minmax(8ch, 1fr) auto minmax(8ch, 1fr);
  gap: 0.5rem;
  align-items: baseline;
  padding: 0.2rem 0;
  border-bottom: 1px solid var(--border);
}
.preview-title {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.preview-before {
  opacity: 0.7;
  text-decoration: line-through;
}
.preview-arrow {
  opacity: 0.5;
}
</style>
