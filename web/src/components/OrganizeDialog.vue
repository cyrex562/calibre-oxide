<script setup lang="ts">
// "Organize files…": move books' files inside the library to where a template
// says. Preview first; only that previewed plan can be run; a finished run can
// be undone. The state machine is in ../library/organize.ts (tested there).

import { confirmDialog } from "./dialogs/confirm";
import Dialog from "./Dialog.vue";
import { computed, onMounted, watch } from "vue";

import { cancelLibraryJob } from "../library/api";
import { describePlan, describeReport, draftProblem, planListIsPartial, type OrganizeController } from "../library/organize";

const props = defineProps<{ controller: OrganizeController; selectedIds: number[] }>();
const emit = defineEmits<{ close: []; changed: [] }>();

const c = props.controller;
const state = c.state;

const busy = computed(() => state.phase === "planning" || state.phase === "running");
const problem = computed(() => draftProblem(state.draft, props.selectedIds));
const canRun = computed(() => state.phase === "planned" && state.plan !== null && state.plan.books > 0 && c.planIsCurrent(props.selectedIds));

// Any edit of the choices voids the preview.
watch(
  () => [state.draft.template, state.draft.scope, state.draft.pathPrefix],
  () => c.edited(),
);

onMounted(() => void c.loadRuns());

async function preview() {
  await c.preview(props.selectedIds);
}

async function run() {
  const plan = state.plan;
  if (!plan) return;
  const sure = await confirmDialog(`Move the files of ${plan.books.toLocaleString("en-US")} books (${plan.files.toLocaleString("en-US")} files) inside the library? Nothing is overwritten, and you can undo this run afterwards.`, { title: "Organize files", confirmLabel: "Move files" });
  if (!sure) return;
  if (await c.run(props.selectedIds)) emit("changed");
}

async function undo(runId: string) {
  if (await c.undo(runId)) emit("changed");
}

function cancelRun() {
  if (state.jobId !== null) void cancelLibraryJob(state.jobId);
}
</script>

<template>
  <Dialog title="Organize files" :busy="busy" show-close size="lg" @close="emit('close')">
    <p class="hint">Files are moved on disk, within the library, to the folders and names the template gives. Nothing is overwritten (a name already taken gets a number), nothing is deleted, and a run can be undone.</p>

    <label class="field">
      Template
      <input v-model="state.draft.template" type="text" :disabled="busy" data-testid="template" spellcheck="false" />
      <span class="hint">For example {author_sort}/{title}: folder Author, file Title.ext</span>
    </label>

    <fieldset class="organize-scope" :disabled="busy">
      <legend>Which books</legend>
      <label><input v-model="state.draft.scope" type="radio" value="selected" :disabled="selectedIds.length === 0" /> The {{ selectedIds.length.toLocaleString("en-US") }} selected book{{ selectedIds.length === 1 ? "" : "s" }}</label>
      <label><input v-model="state.draft.scope" type="radio" value="folder" /> Books in a folder</label>
      <input v-if="state.draft.scope === 'folder'" v-model="state.draft.pathPrefix" type="text" placeholder="Folder, relative to the library (e.g. Fiction/Old)" data-testid="path-prefix" />
      <label><input v-model="state.draft.scope" type="radio" value="all" /> Every book in the library</label>
    </fieldset>

    <div class="organize-buttons">
      <button type="button" :disabled="busy || problem !== null" data-testid="preview" @click="preview">{{ state.phase === "planning" ? "Previewing…" : "Preview" }}</button>
    </div>
    <p v-if="problem" class="hint">{{ problem }}</p>
    <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>

    <section v-if="state.plan" class="organize-plan" aria-label="Preview" data-testid="plan">
      <p role="status">{{ describePlan(state.plan) }}</p>
      <p v-if="state.plan.books === 0" class="hint">Nothing to move.</p>
      <ul v-else class="organize-moves">
        <li v-for="(m, i) in state.plan.moves" :key="i">
          <span class="title">{{ m.title }}</span> <span class="hint">({{ m.format }})</span>
          <span v-if="m.suffixed" class="warn" title="The target name was taken, so the file gets a number"> numbered</span>
          <div class="path">{{ m.from }} → {{ m.to }}</div>
        </li>
      </ul>
      <p v-if="planListIsPartial(state.plan)" class="hint">Showing the first {{ state.plan.moves.length.toLocaleString("en-US") }} of {{ state.plan.files.toLocaleString("en-US") }} files.</p>
      <details v-if="state.plan.skipped.length > 0">
        <summary>{{ state.plan.skipped.length }} skipped</summary>
        <ul>
          <li v-for="s in state.plan.skipped.slice(0, 50)" :key="s.book_id">Book {{ s.book_id }}: {{ s.reason }}</li>
        </ul>
      </details>
      <div class="organize-buttons">
        <button type="button" class="primary" :disabled="busy || !canRun" data-testid="run-plan" @click="run">Organize {{ state.plan.books.toLocaleString("en-US") }} books</button>
      </div>
    </section>

    <div v-if="state.phase === 'running'" class="organize-buttons">
      <p class="hint" role="status">{{ state.progress }}</p>
      <button v-if="state.jobId !== null" type="button" @click="cancelRun">Cancel</button>
    </div>

    <section v-if="state.report" class="organize-done" aria-label="Result" data-testid="report">
      <p role="status">{{ describeReport(state.report) }}</p>
      <ul v-if="state.report.failed.length > 0">
        <li v-for="f in state.report.failed.slice(0, 5)" :key="f.book_id">Book {{ f.book_id }}: {{ f.reason }}</li>
      </ul>
      <button type="button" :disabled="busy" data-testid="undo-run" @click="undo(state.report.run_id)">Undo this run</button>
    </section>
    <p v-if="state.undone" class="hint" role="status">{{ state.undone }}</p>

    <section v-if="state.runs.length > 0 && !state.report" class="organize-runs">
      <h4>Earlier runs</h4>
      <ul>
        <li v-for="r in state.runs.slice(0, 5)" :key="r.run_id">
          {{ r.books.toLocaleString("en-US") }} book{{ r.books === 1 ? "" : "s" }} · <span class="hint">{{ new Date(r.created_at * 1000).toLocaleString() }}</span>
          <button type="button" :disabled="busy" @click="undo(r.run_id)">Undo</button>
        </li>
      </ul>
    </section>
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
.warn {
  color: var(--warning);
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
fieldset {
  border: 1px solid var(--border);
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.5rem 0.75rem;
}
.organize-buttons {
  display: flex;
  gap: 0.5rem;
  align-items: center;
}
.organize-plan {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.6rem 0.8rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.organize-moves {
  margin: 0;
  padding-left: 1.1rem;
  max-height: 30vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
}
.path {
  font-family: monospace;
  font-size: var(--fs-body);
  overflow-wrap: anywhere;
  color: var(--fg-muted);
}
.organize-done {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  align-items: flex-start;
}
.organize-runs ul {
  margin: 0.3rem 0 0;
  padding-left: 1.1rem;
}
</style>
