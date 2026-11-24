<script setup lang="ts">
// "Export a copy…": a travel copy (a library you take with you, refreshed later)
// or a backup mirror (an exact copy, e.g. on the NAS). Preview first; only that
// previewed plan can be run. The state machine is in ../library/mirror.ts
// (tested there).

import { confirmDialog } from "./dialogs/confirm";
import Dialog from "./Dialog.vue";
import { computed, onMounted, watch } from "vue";

import { cancelLibraryJob } from "../library/api";
import type { MirrorAction } from "../library/api";
import { conflictText, conflictsNote, describePlan, needsConfirmation, describeReport, describeScope, describeTarget, draftProblem, formatBytes, listIsPartial, MODE_LABELS, networkNote, reachableLabel, type MirrorController } from "../library/mirror";

const props = withDefaults(defineProps<{ controller: MirrorController; selectedIds: number[]; /** The books the list shows now, when a search or filter is active. */ searchIds?: number[]; searchActive?: boolean }>(), { searchIds: () => [], searchActive: false });
const emit = defineEmits<{ close: [] }>();

const c = props.controller;
const state = c.state;

const busy = computed(() => state.phase === "planning" || state.phase === "running");
// The books a "selected" or "search" scope covers (a refresh of an earlier copy reuses the ones it held).
const scopeIds = computed(() => (state.draft.scope === "search" ? props.searchIds : props.selectedIds));
const ids = computed(() => state.rememberedIds ?? scopeIds.value);
const problem = computed(() => draftProblem(state.draft, ids.value));
const canRun = computed(() => state.phase === "planned" && state.plan !== null && c.planIsCurrent(scopeIds.value));
const isTravel = computed(() => state.draft.mode === "travel");

// Any edit of the choices voids the preview.
watch(
  () => [state.draft.mode, state.draft.target, state.draft.scope, state.draft.folderPrefix, state.draft.removeMissing, state.draft.overwriteChanged],
  () => c.edited(),
);

onMounted(() => void c.loadTargets());

const sections: { action: MirrorAction; title: string }[] = [
  { action: "conflict", title: "Conflicts (skipped)" },
  { action: "copy_new", title: "New" },
  { action: "update", title: "Updated" },
  { action: "remove", title: "To remove (moved to .mirror-trash in the copy)" },
];

async function preview() {
  await c.preview(scopeIds.value);
}

async function run() {
  const plan = state.plan;
  if (!plan) return;
  // Only adding files to a new place loses nothing, so the Copy click is the decision. Overwrites and removals are asked about.
  if (needsConfirmation(plan, state.draft)) {
    const warn = state.draft.overwriteChanged ? " Files changed in the copy will be overwritten." : "";
    const gone = plan.totals.remove.files > 0 ? ` ${plan.totals.remove.files.toLocaleString("en-US")} file${plan.totals.remove.files === 1 ? "" : "s"} no longer in the library will be moved to .mirror-trash in the copy.` : "";
    const sure = await confirmDialog(`Copy ${(plan.totals.copy_new.files + plan.totals.update.files).toLocaleString("en-US")} files (${formatBytes(plan.totals.bytes_to_copy)}) to ${plan.target}?${warn}${gone} Nothing in the library changes.`, { title: "Export a copy", confirmLabel: "Copy" });
    if (!sure) return;
  }
  await c.run(scopeIds.value);
}

function cancelRun() {
  if (state.jobId !== null) void cancelLibraryJob(state.jobId);
}

const canBrowse = computed(() => c.canBrowse());
</script>

<template>
  <Dialog title="Export a copy" :busy="busy" show-close size="lg" @close="emit('close')">
    <p class="hint">Copies the library to another folder or drive. The library itself is not changed, and nothing in the copy is deleted: files the library no longer has are moved to .mirror-trash inside the copy, never erased.</p>

    <section v-if="state.targets.length > 0" class="mirror-targets" aria-label="Earlier copies" data-testid="targets">
      <h4>Earlier copies</h4>
      <ul>
        <li v-for="t in state.targets" :key="t.target" data-testid="target-row">
          <div class="target-line">
            <span class="path">{{ t.target }}</span>
            <span :class="['badge', t.reachable ? 'ok' : 'off']" data-testid="reachable">{{ reachableLabel(t) }}</span>
          </div>
          <div class="hint">{{ t.mode === "travel" ? "Travel copy" : "Backup mirror" }} · {{ describeScope(t.scope) }} · {{ describeTarget(t) }}</div>
          <div class="row">
            <button type="button" :disabled="busy || !t.reachable" data-testid="refresh-target" @click="c.refresh(t, scopeIds)">Refresh</button>
            <button type="button" :disabled="busy" data-testid="forget-target" title="Forget this folder. Its files stay where they are." @click="c.forget(t.target)">Forget</button>
          </div>
        </li>
      </ul>
    </section>

    <fieldset class="mirror-mode" :disabled="busy">
      <legend>What kind of copy</legend>
      <label v-for="mode in (['travel', 'mirror'] as const)" :key="mode">
        <input v-model="state.draft.mode" type="radio" :value="mode" :data-testid="`mode-${mode}`" />
        {{ MODE_LABELS[mode].title }}
        <span class="hint block">{{ MODE_LABELS[mode].hint }}</span>
      </label>
    </fieldset>

    <label class="field">
      Folder to put the copy in
      <span class="row">
        <input v-model="state.draft.target" type="text" :disabled="busy" data-testid="target" spellcheck="false" placeholder="D:\Library or \\nas\share\Library" />
        <button v-if="canBrowse" type="button" :disabled="busy" data-testid="browse" @click="c.browse()">Browse…</button>
      </span>
      <span class="hint">An empty or new folder, or one an earlier copy made. Not inside the library.</span>
    </label>

    <fieldset v-if="isTravel" class="mirror-scope" :disabled="busy">
      <legend>Which books</legend>
      <label><input v-model="state.draft.scope" type="radio" value="selected" :disabled="(state.rememberedIds ?? selectedIds).length === 0" /> The {{ (state.rememberedIds ?? selectedIds).length.toLocaleString("en-US") }} selected book{{ (state.rememberedIds ?? selectedIds).length === 1 ? "" : "s" }}</label>
      <label v-if="searchActive"><input v-model="state.draft.scope" type="radio" value="search" data-testid="scope-search" :disabled="searchIds.length === 0" /> The {{ searchIds.length.toLocaleString("en-US") }} book{{ searchIds.length === 1 ? "" : "s" }} in the current search or filter</label>
      <label><input v-model="state.draft.scope" type="radio" value="folder" /> Books in a folder</label>
      <input v-if="state.draft.scope === 'folder'" v-model="state.draft.folderPrefix" type="text" placeholder="Folder, relative to the library (e.g. Fiction/Old)" data-testid="folder-prefix" />
      <label><input v-model="state.draft.scope" type="radio" value="all" /> Every book in the library</label>
    </fieldset>

    <fieldset class="mirror-options" :disabled="busy">
      <legend>Options</legend>
      <label><input v-model="state.draft.removeMissing" type="checkbox" data-testid="remove-missing" /> Move files the library no longer has to .mirror-trash in the copy (nothing is deleted)</label>
      <label><input v-model="state.draft.overwriteChanged" type="checkbox" data-testid="overwrite-changed" /> Overwrite files changed in the copy</label>
      <p v-if="state.draft.overwriteChanged" class="warn" role="alert" data-testid="overwrite-warning">Edits made in the copy since the last refresh will be lost, including changes made to a travel copy's library on another computer.</p>
    </fieldset>

    <p v-if="problem && state.draft.target.trim() !== ''" class="hint">{{ problem }}</p>
    <p v-if="state.notice" class="hint" role="status">{{ state.notice }}</p>
    <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>

    <section v-if="state.plan" class="mirror-plan" aria-label="Preview" data-testid="plan">
      <p role="status" data-testid="plan-summary">{{ describePlan(state.plan) }}</p>
      <p class="hint">{{ state.plan.books.toLocaleString("en-US") }} book{{ state.plan.books === 1 ? "" : "s" }} in the copy · {{ state.plan.scope }} · {{ state.plan.new_target ? "a new copy" : "refreshing an earlier copy" }}</p>
      <p v-if="networkNote(state.plan)" class="hint" data-testid="network-note">{{ networkNote(state.plan) }}</p>
      <p v-if="conflictsNote(state.plan)" class="warn" role="alert" data-testid="conflicts-note">{{ conflictsNote(state.plan) }}</p>
      <p v-if="state.plan.missing_sources.count > 0" class="hint">{{ state.plan.missing_sources.count.toLocaleString("en-US") }} file{{ state.plan.missing_sources.count === 1 ? " the library lists is" : "s the library lists are" }} not on disk and cannot be copied.</p>
      <template v-for="s in sections" :key="s.action">
        <details v-if="state.plan.totals[s.action].files > 0" :open="s.action === 'conflict'">
          <summary>{{ s.title }}: {{ state.plan.totals[s.action].files.toLocaleString("en-US") }} ({{ formatBytes(state.plan.totals[s.action].bytes) }})</summary>
          <ul class="mirror-files">
            <li v-for="(item, i) in state.plan.items[s.action]" :key="i">
              <template v-if="s.action === 'conflict'">{{ conflictText(item) }}</template>
              <template v-else><span class="path">{{ item.path }}</span> <span class="hint">({{ formatBytes(item.bytes) }})</span></template>
            </li>
          </ul>
          <p v-if="listIsPartial(state.plan, s.action)" class="hint">Showing the first {{ state.plan.items[s.action].length.toLocaleString("en-US") }}.</p>
        </details>
      </template>
    </section>

    <p v-if="state.phase === 'running' || state.phase === 'planning'" class="hint" role="status" data-testid="progress">{{ state.progress }}</p>

    <section v-if="state.report" class="mirror-done" aria-label="Result" data-testid="report">
      <p role="status">{{ describeReport(state.report) }}</p>
      <ul v-if="state.report.conflicts.length > 0">
        <li v-for="f in state.report.conflicts.slice(0, 5)" :key="f.path">{{ f.path }}: {{ f.message }}</li>
      </ul>
      <ul v-if="state.report.failures.length > 0">
        <li v-for="f in state.report.failures.slice(0, 5)" :key="f.path">{{ f.path }}: {{ f.message }}</li>
      </ul>
      <p v-if="state.report.edits_set_aside > 0" class="hint">{{ state.report.edits_set_aside }} change{{ state.report.edits_set_aside === 1 ? "" : "s" }} made in the copy were set aside in {{ state.report.trash }}.</p>
    </section>
    <template #footer>
      <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      <button v-if="(state.phase === 'running' || state.phase === 'planning') && state.jobId !== null" type="button" data-testid="cancel" @click="cancelRun">Cancel</button>
      <button type="button" :disabled="busy || problem !== null" data-testid="preview" @click="preview">{{ state.phase === "planning" ? "Previewing…" : "Preview" }}</button>
      <button v-if="state.plan" type="button" class="primary" :disabled="busy || !canRun" data-testid="run-plan" @click="run">{{ state.plan.new_target ? "Copy" : "Refresh" }}</button>
    </template>
  </Dialog>
</template>

<style scoped>
h4 {
  margin: 0;
}
.hint {
  color: var(--fg-muted);
  font-size: var(--fs-body);
  margin: 0;
}
.hint.block {
  display: block;
  margin-left: 1.4rem;
}
.warn {
  color: var(--warning);
  margin: 0;
}
.error {
  color: var(--danger);
  margin: 0;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}
.row {
  display: flex;
  gap: 0.5rem;
  align-items: center;
}
.row input[type="text"] {
  flex: 1;
}
fieldset {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.5rem 0.75rem;
}
.mirror-targets ul {
  margin: 0.3rem 0 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}
.target-line {
  display: flex;
  gap: 0.5rem;
  align-items: baseline;
  justify-content: space-between;
}
.badge {
  font-size: var(--fs-body);
  border-radius: var(--radius-pill);
  padding: 0 0.5rem;
  border: 1px solid var(--border);
}
.badge.ok {
  color: var(--success);
}
.badge.off {
  color: var(--fg-muted);
}
.mirror-plan {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.6rem 0.8rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.mirror-files {
  margin: 0.3rem 0 0;
  padding-left: 1.1rem;
  max-height: 25vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}
.path {
  font-family: monospace;
  font-size: var(--fs-body);
  overflow-wrap: anywhere;
}
.mirror-done {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}
</style>
