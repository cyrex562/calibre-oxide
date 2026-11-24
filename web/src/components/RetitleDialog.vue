<script setup lang="ts">
// "Fix titles…": find books whose stored title is junk, suggest a clean one,
// let the user approve or edit each, then in one run set the titles, write them
// into the files and rename the files. A finished run can be undone. The state
// machine is in ../library/retitle.ts (tested there).

import { confirmDialog } from "./dialogs/confirm";
import Dialog from "./Dialog.vue";
import { computed, onMounted, ref, watchEffect } from "vue";

import { cancelLibraryJob } from "../library/api";
import type { RetitleFilter } from "../library/retitle";
import { checkState, describeConfirm, describeFailure, describeReport, describeRows, describeSkip, pageCount, REASON_LABEL, rowProblem, visibleRows, type RetitleController } from "../library/retitle";

const props = defineProps<{ controller: RetitleController; selectedIds: number[] }>();
const emit = defineEmits<{ close: []; changed: [] }>();

const c = props.controller;
const state = c.state;

const busy = computed(() => state.phase === "finding" || state.phase === "running");
const filtered = computed(() => visibleRows(state.rows, state.filter));
const shown = computed(() => c.pageRows());
const pages = computed(() => pageCount(filtered.value.length));
const headerState = computed(() => checkState(filtered.value));
const checked = computed(() => c.checkedCount());
const rowsLine = computed(() => describeRows(filtered.value.length, state.rows.length, checked.value, state.serverTotal, state.truncated));
const filters: { value: RetitleFilter; label: string }[] = [
  { value: "all", label: "All reasons" },
  { value: "junk", label: REASON_LABEL.junk },
  { value: "template_leftover", label: REASON_LABEL.template_leftover },
  { value: "no_suggestion", label: REASON_LABEL.no_suggestion },
  { value: "ok", label: REASON_LABEL.ok },
];

// The header checkbox is tri-state, which only the DOM property can show.
const headerBox = ref<HTMLInputElement | null>(null);
watchEffect(() => {
  if (headerBox.value) headerBox.value.indeterminate = headerState.value === "some";
});

onMounted(() => void c.loadRuns());

async function find() {
  await c.find(props.selectedIds);
}

async function run() {
  const sure = await confirmDialog(`Apply ${describeConfirm(checked.value, { embed: state.embed, renameFiles: state.renameFiles, renameTemplate: state.renameTemplate })}`, { title: "Fix titles", confirmLabel: "Apply" });
  if (!sure) return;
  if (await c.apply()) emit("changed");
}

async function undo(runId: string) {
  if (await c.undo(runId)) emit("changed");
}

function cancelRun() {
  if (state.jobId !== null) void cancelLibraryJob(state.jobId);
}
</script>

<template>
  <Dialog title="Fix titles" :busy="busy" show-close size="lg" @close="emit('close')">
    <p class="hint">
      Finds books whose title is junk (a print job, "Microsoft Word - …", a bare file name, or one title copied onto many different files) and suggests a clean one. Check or edit each, then apply: the titles are set, optionally written into the files, and the files optionally
      renamed. A run can be undone.
    </p>
    <p class="hint" data-testid="scope">{{ state.scope === "selected" ? `Looking at the ${selectedIds.length.toLocaleString("en-US")} selected book${selectedIds.length === 1 ? "" : "s"}.` : "Looking at every book in the library." }}</p>

    <fieldset class="retitle-options" :disabled="busy">
      <legend>When applying</legend>
      <label title="Rewrites each book file (PDF, EPUB, …) so its own metadata carries the title. The file's contents change, so its checksum does too; an undo writes the old title back, but the bytes will not be identical to before."
        ><input v-model="state.embed" type="checkbox" data-testid="embed" /> Write the title into the files</label
      >
      <label><input v-model="state.renameFiles" type="checkbox" data-testid="rename" /> Rename files</label>
      <input v-if="state.renameFiles" v-model="state.renameTemplate" type="text" spellcheck="false" aria-label="File name template" data-testid="template" />
      <span v-if="state.renameFiles" class="hint">A template such as {title} or {title} - {authors}. A name already taken gets a number.</span>
    </fieldset>

    <p v-if="c.proposedIsStale()" class="hint" role="status">The file names below are for the old template; find again to refresh them. They follow the template when applied.</p>
    <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>

    <section v-if="state.phase === 'ready'" class="retitle-table" aria-label="Titles to fix" data-testid="table">
      <p v-if="state.rows.length === 0" role="status" class="hint">No titles need fixing.</p>
      <template v-else>
        <div class="retitle-toolbar">
          <select v-model="state.filter" aria-label="Filter by reason" data-testid="filter" @change="c.setFilter(state.filter)">
            <option v-for="f in filters" :key="f.value" :value="f.value">{{ f.label }}</option>
          </select>
          <span role="status" class="hint" data-testid="count">{{ rowsLine }}</span>
        </div>
        <table>
          <thead>
            <tr>
              <th><input ref="headerBox" type="checkbox" :checked="headerState === 'all'" aria-label="Select all" data-testid="select-all" @change="c.setAllChecked(($event.target as HTMLInputElement).checked)" /></th>
              <th>Current title</th>
              <th>New title</th>
              <th>Why</th>
              <th>File</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="r in shown" :key="r.bookId" data-testid="row">
              <td><input v-model="r.checked" type="checkbox" :aria-label="`Fix book ${r.bookId}`" /></td>
              <td class="muted">{{ r.currentTitle }}</td>
              <td>
                <input v-model="r.title" type="text" :class="{ invalid: rowProblem(r) }" :placeholder="r.reason === 'no_suggestion' ? 'Type a title' : ''" :title="rowProblem(r) ?? ''" />
              </td>
              <td>
                <span class="badge" :class="r.reason">{{ REASON_LABEL[r.reason] }}</span>
                <span v-if="!r.confident && r.reason !== 'ok' && r.reason !== 'no_suggestion'" class="badge unsure" title="Suggested from a file name that looks like a catalogue number, so not selected: check it first">check</span>
              </td>
              <td class="path">
                {{ r.currentFile }}<template v-if="state.renameFiles && r.proposedFile && r.proposedFile !== r.currentFile"> → {{ r.proposedFile }}</template>
              </td>
            </tr>
          </tbody>
        </table>
        <div v-if="pages > 1" class="retitle-pager">
          <button type="button" :disabled="state.page === 0" @click="c.setPage(state.page - 1)">Previous</button>
          <span class="hint">Page {{ state.page + 1 }} of {{ pages }}</span>
          <button type="button" :disabled="state.page >= pages - 1" @click="c.setPage(state.page + 1)">Next</button>
        </div>
      </template>
    </section>

    <p v-if="state.phase === 'running'" class="hint" role="status">{{ state.progress }}</p>

    <section v-if="state.report" class="retitle-done" aria-label="Result" data-testid="report">
      <p role="status">{{ describeReport(state.report) }}</p>
      <ul v-if="state.report.skipped.length > 0">
        <li v-for="s in state.report.skipped.slice(0, 5)" :key="s.book_id">Book {{ s.book_id }} skipped: {{ describeSkip(s.reason) }}</li>
      </ul>
      <ul v-if="state.report.failed.length > 0" class="error">
        <li v-for="(f, i) in state.report.failed.slice(0, 5)" :key="i">Book {{ f.book_id }}: {{ describeFailure(f) }}</li>
      </ul>
      <button type="button" :disabled="busy" data-testid="undo-run" @click="undo(state.report.run_id)">Undo this run</button>
    </section>
    <p v-if="state.undone" class="hint" role="status">{{ state.undone }}</p>

    <section v-if="state.runs.length > 0 && !state.report" class="retitle-runs">
      <h4>Earlier runs</h4>
      <ul>
        <li v-for="r in state.runs.slice(0, 5)" :key="r.run_id">
          {{ r.books.toLocaleString("en-US") }} book{{ r.books === 1 ? "" : "s" }} · <span class="hint">{{ new Date(r.created_at * 1000).toLocaleString() }}</span>
          <button type="button" :disabled="busy" @click="undo(r.run_id)">Undo</button>
        </li>
      </ul>
    </section>
    <template #footer>
      <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      <button v-if="state.phase === 'running' && state.jobId !== null" type="button" @click="cancelRun">Cancel</button>
      <button type="button" :disabled="busy" data-testid="find" @click="find">{{ state.phase === "finding" ? "Looking…" : state.rows.length > 0 ? "Find again" : "Find titles to fix" }}</button>
      <button v-if="state.phase === 'ready' && state.rows.length > 0" type="button" class="primary" :disabled="busy || !c.canApply()" data-testid="apply" @click="run">Fix {{ checked.toLocaleString("en-US") }} title{{ checked === 1 ? "" : "s" }}</button>
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
.error {
  color: var(--danger);
  margin: 0;
}
fieldset {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.5rem 0.75rem;
}
.retitle-toolbar,
.retitle-pager {
  display: flex;
  gap: 0.5rem;
  align-items: center;
  flex-wrap: wrap;
}
.retitle-table {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.6rem 0.8rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  min-width: 0;
}
table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--fs-body);
}
th {
  text-align: left;
  font-weight: 600;
  padding: 0.25rem 0.4rem;
}
td {
  padding: 0.2rem 0.4rem;
  vertical-align: top;
}
td input[type="text"] {
  width: 100%;
  min-width: 14rem;
  box-sizing: border-box;
}
td input.invalid {
  border-color: var(--danger);
}
.muted {
  color: var(--fg-muted);
  overflow-wrap: anywhere;
  max-width: 18rem;
}
.path {
  font-family: monospace;
  overflow-wrap: anywhere;
  color: var(--fg-muted);
  max-width: 22rem;
}
.badge {
  display: inline-block;
  padding: 0 0.4rem;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  font-size: var(--fs-body);
  white-space: nowrap;
}
.badge.junk {
  color: var(--warning);
}
.badge.template_leftover {
  color: #8250df;
}
.badge.no_suggestion {
  color: var(--danger);
}
.badge.unsure {
  margin-left: 0.25rem;
  color: var(--fg-muted);
}
.retitle-done {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  align-items: flex-start;
}
.retitle-runs ul {
  margin: 0.3rem 0 0;
  padding-left: 1.1rem;
}
</style>
