<script setup lang="ts">
// "Resolve…": act on many duplicate groups at once.
//
// Choose which tiers to trust, a minimum confidence and an optional folder,
// and what to do with the extra copies. The server previews the plan (the
// keeper of every group and why, the books affected, the space freed); only
// that previewed plan can be run, and a finished run can be undone.
//
// The state machine is in ../library/resolve.ts (tested there).

import { confirmDialog } from "./dialogs/confirm";
import Dialog from "./Dialog.vue";
import { computed, onMounted, watch } from "vue";

import { cancelLibraryJob } from "../library/api";
import type { ResolveAction } from "../library/api";
import { ALL_REASONS, confidencePercent, describePlanTotals, formatBytes, isMetadataOnly, reasonLabel } from "../library/duplicates";
import type { DuplicatesStore } from "../library/duplicatesStore";
import { actionNote, describeReport, type ResolveController } from "../library/resolve";

const props = defineProps<{ store: DuplicatesStore; controller: ResolveController }>();
const emit = defineEmits<{ close: []; changed: [] }>();

const c = props.controller;
const state = c.state;
const CONFIDENCES = [
  { value: 0, label: "any confidence" },
  { value: 0.7, label: "70% or more" },
  { value: 0.8, label: "80% or more" },
  { value: 0.9, label: "90% or more" },
  { value: 0.95, label: "95% or more" },
  { value: 1, label: "certain (100%)" },
];
const ACTIONS: { value: ResolveAction; label: string }[] = [
  { value: "keep_best_trash_rest", label: "Keep the best copy, move the others to the trash" },
  { value: "merge_as_formats", label: "Keep the best copy, merge the others into it" },
];

const myKeepers = computed(() => props.store.state.keepers);
const pickedCount = computed(() => Object.keys(myKeepers.value).length);
const busy = computed(() => state.phase === "planning" || state.phase === "running");
const shownGroups = computed(() => (state.plan?.groups ?? []).slice(0, 12));

// Any edit of the choices voids the preview.
watch(
  () => [state.draft.tiers.join(","), state.draft.minConfidence, state.draft.pathPrefix, state.draft.action, state.draft.keepDuplicateFormats, state.draft.useMyKeepers],
  () => c.edited(),
);

onMounted(() => void c.loadRuns());

function toggleTier(reason: (typeof ALL_REASONS)[number]) {
  const tiers = state.draft.tiers;
  state.draft.tiers = tiers.includes(reason) ? tiers.filter((t) => t !== reason) : [...tiers, reason];
}

async function preview() {
  await c.preview(myKeepers.value);
}

async function run() {
  const plan = state.plan;
  if (!plan) return;
  const merging = state.draft.action === "merge_as_formats";
  const sure = await confirmDialog(
    merging
      ? `Merge ${plan.totals.books.toLocaleString("en-US")} books into their keepers? The merged books are moved to the trash, but what was merged into each keeper cannot be taken out again by undoing the run.`
      : `Move ${plan.totals.books.toLocaleString("en-US")} books from ${plan.totals.groups.toLocaleString("en-US")} groups to the library trash? You can undo this run afterwards.`,
    { title: "Resolve duplicates", confirmLabel: merging ? "Merge" : "Move to trash", danger: true },
  );
  if (!sure) return;
  const report = await c.run(myKeepers.value);
  if (report) {
    await props.store.refill();
    emit("changed");
  }
}

async function undo(runId: string) {
  if (await c.undo(runId)) {
    emit("changed");
    // The restored books make new groups: they come back with a rescan.
    void props.store.scan();
  }
}

function cancelRun() {
  // The job is cancelled from its own route; the dialog just stops waiting for more.
  if (state.jobId !== null) void cancelLibraryJob(state.jobId);
}

</script>

<template>
  <Dialog title="Resolve duplicates" :busy="busy" show-close size="lg" @close="emit('close')">
    <p class="hint">Pick which kinds of match to trust. Nothing changes until you have seen the plan and run it.</p>

    <fieldset class="resolve-tiers" :disabled="busy">
      <legend>Include groups found by</legend>
      <label v-for="r in ALL_REASONS" :key="r" :class="{ risky: isMetadataOnly(r) }">
        <input type="checkbox" :checked="state.draft.tiers.includes(r)" :data-testid="`tier-${r}`" @change="toggleTier(r)" />
        {{ reasonLabel(r) }}
        <span class="hint">({{ (store.state.facets[r] ?? 0).toLocaleString("en-US") }})</span>
        <span v-if="isMetadataOnly(r)" class="hint">— metadata only: two different books can match, so check before ticking</span>
      </label>
    </fieldset>

    <div class="resolve-row">
      <label>
        Confidence
        <select v-model.number="state.draft.minConfidence" :disabled="busy" aria-label="Minimum confidence">
          <option v-for="o in CONFIDENCES" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </label>
      <label class="grow">
        Only under folder
        <input v-model="state.draft.pathPrefix" :disabled="busy" placeholder="e.g. Fiction/Unsorted (optional)" />
      </label>
    </div>

    <fieldset class="resolve-actions" :disabled="busy">
      <legend>What to do with the other copies</legend>
      <label v-for="a in ACTIONS" :key="a.value">
        <input v-model="state.draft.action" type="radio" name="resolve-action" :value="a.value" />
        {{ a.label }}
      </label>
      <p :class="['hint', { warn: state.draft.action === 'merge_as_formats' }]" data-testid="action-note">{{ actionNote(state.draft.action) }}</p>
      <label v-if="state.draft.action === 'merge_as_formats'">
        <input v-model="state.draft.keepDuplicateFormats" type="checkbox" />
        Keep a differing copy of a format as a data file instead of discarding it
      </label>
      <label v-if="pickedCount > 0">
        <input v-model="state.draft.useMyKeepers" type="checkbox" />
        Use the {{ pickedCount }} copy choice{{ pickedCount === 1 ? "" : "s" }} I made in the list
      </label>
    </fieldset>

    <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>

    <div class="resolve-buttons">
      <button type="button" :disabled="busy || state.draft.tiers.length === 0" data-testid="preview-plan" @click="preview">{{ state.phase === "planning" ? "Working out the plan…" : "Preview plan…" }}</button>
    </div>

    <section v-if="state.plan" class="resolve-plan" aria-label="The plan" data-testid="plan">
      <p>
        <strong>{{ describePlanTotals(state.plan.totals, state.plan.action) }}</strong>
      </p>
      <p v-if="state.plan.totals.groups === 0" class="hint">No groups match these choices.</p>
      <ul class="resolve-sample">
        <li v-for="g in shownGroups" :key="g.group_id">
          <span class="hint">{{ reasonLabel(g.reason) }} · {{ confidencePercent(g.confidence) }}</span><br />
          keep <strong>{{ g.keeper.title }}</strong> <span class="hint">({{ g.keeper_reason }})</span>
          <template v-if="g.others.length">
            — {{ state.plan.action === "merge_as_formats" ? "merge in" : "trash" }}
            {{ g.others.map((o) => o.title + (o.size ? ` (${formatBytes(o.size)})` : "")).join(", ") }}
          </template>
        </li>
        <li v-if="state.plan.groups.length > shownGroups.length" class="hint">…and {{ (state.plan.groups.length - shownGroups.length).toLocaleString("en-US") }} more groups</li>
      </ul>
      <p v-for="s in state.plan.skipped" :key="s.group_id" class="hint">Skipped a group: {{ s.reason }}.</p>
      <p v-if="!state.plan.fully_restorable" class="hint warn">{{ state.plan.note }}</p>
      <div class="resolve-buttons">
        <button type="button" class="primary" :disabled="busy || state.plan.totals.groups === 0" data-testid="run-plan" @click="run">
          {{ state.phase === "running" ? "Running…" : `Resolve ${state.plan.totals.groups.toLocaleString("en-US")} groups` }}
        </button>
        <button v-if="state.phase === 'running'" type="button" @click="cancelRun">Cancel</button>
      </div>
      <p v-if="state.progress" class="hint" role="status">{{ state.progress }}</p>
    </section>

    <section v-if="state.report" class="resolve-done" aria-label="Result" data-testid="report">
      <p role="status">{{ describeReport(state.report) }}</p>
      <button type="button" data-testid="undo-run" @click="undo(state.report.run_id)">Undo this run</button>
      <p v-if="!state.report.fully_restorable" class="hint warn">Undo brings the trashed books back, but what was merged into the kept copies stays there.</p>
    </section>
    <p v-if="state.undone" class="hint" role="status">{{ state.undone }}</p>

    <section v-if="state.runs.length > 0 && !state.report" class="resolve-runs">
      <h4>Earlier runs still in the trash</h4>
      <ul>
        <li v-for="r in state.runs" :key="r.run_id">
          {{ r.books }} book{{ r.books === 1 ? "" : "s" }} · {{ formatBytes(r.bytes) }} · <span class="hint">{{ new Date(r.deleted_at * 1000).toLocaleString() }}</span>
          <button type="button" :disabled="busy" @click="undo(r.run_id)">Undo this run</button>
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
fieldset {
  border: 1px solid var(--border);
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.5rem 0.75rem;
}
.risky {
  color: var(--warning);
}
.resolve-row {
  display: flex;
  gap: 1rem;
  align-items: flex-end;
}
.resolve-row label {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}
.resolve-row .grow {
  flex: 1;
}
.resolve-buttons {
  display: flex;
  gap: 0.5rem;
}
.resolve-plan {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.6rem 0.8rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.resolve-sample {
  margin: 0;
  padding-left: 1.1rem;
  max-height: 30vh;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
}
.resolve-done {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  align-items: flex-start;
}
.resolve-runs ul {
  margin: 0.3rem 0 0;
  padding-left: 1.1rem;
}
</style>
