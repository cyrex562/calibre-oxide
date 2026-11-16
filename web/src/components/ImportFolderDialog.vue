<script setup lang="ts">
// "Add folder": copy a folder tree into the library, as a job with progress.
//
// The folder is chosen with the desktop app's own picker (a path on this
// machine) or typed. Nothing in the source folder is changed: files are
// copied, verified, and laid out as Author/Title (id). Cancel stops after the
// batch in flight; importing the same folder again carries on where it left
// off. Choices and wording: ../library/importFolder.ts.

import { computed, ref } from "vue";

import { cancelLibraryJob, describeJobProgress, importFolder } from "../library/api";
import { DEFAULT_IMPORT, DUPLICATE_POLICIES, IMPORT_MODES, describeImport, importProblems, validateSource, type DuplicatePolicy, type ImportMode, type ImportReport } from "../library/importFolder";
import { isTauri, tauriInvoke } from "../tauri";

const props = defineProps<{ initialSource?: string }>();
const emit = defineEmits<{ close: []; imported: [] }>();

const source = ref(props.initialSource ?? "");
const mode = ref<ImportMode>(DEFAULT_IMPORT.mode);
const policy = ref<DuplicatePolicy>(DEFAULT_IMPORT.policy);
const recursive = ref(DEFAULT_IMPORT.recursive);

const busy = ref(false);
const jobId = ref<number | null>(null);
const progress = ref<string | null>(null);
const error = ref<string | null>(null);
const report = ref<ImportReport | null>(null);

const problem = computed(() => (source.value === "" ? null : validateSource(source.value)));
const modeHelp = computed(() => IMPORT_MODES.find((m) => m.value === mode.value)?.help ?? "");
const policyHelp = computed(() => DUPLICATE_POLICIES.find((p) => p.value === policy.value)?.help ?? "");
const summary = computed(() => (report.value ? describeImport(report.value) : []));
const problems = computed(() => (report.value ? importProblems(report.value) : []));

async function browse() {
  try {
    const picked = await tauriInvoke<string | null>("choose_import_folder");
    if (picked) source.value = picked;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function start() {
  const bad = validateSource(source.value);
  if (bad) {
    error.value = bad;
    return;
  }
  busy.value = true;
  error.value = null;
  report.value = null;
  progress.value = null;
  try {
    report.value = await importFolder(
      { source: source.value, mode: mode.value, policy: policy.value, recursive: recursive.value },
      {
        onStart: (id) => (jobId.value = id),
        onProgress: (job) => (progress.value = describeJobProgress(job.progress)),
      },
    );
    emit("imported");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
    jobId.value = null;
    progress.value = null;
  }
}

/** Stops after the batch in flight; what is already in stays, and a rerun continues. */
async function cancel() {
  if (jobId.value === null) return;
  try {
    await cancelLibraryJob(jobId.value);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div class="manage-backdrop" @click.self="!busy && emit('close')">
    <div class="manage-panel import-panel" role="dialog" aria-label="Add folder">
      <h3>Add folder</h3>
      <p class="hint">Copies the books in a folder into this library, filing each as Author/Title. The folder itself is never changed.</p>

      <label class="field">
        Folder
        <span class="import-source">
          <input v-model="source" placeholder="Full path of the folder, e.g. D:\Books\Inbox" :disabled="busy" spellcheck="false" />
          <button v-if="isTauri()" type="button" :disabled="busy" @click="browse">Browse…</button>
        </span>
      </label>
      <p v-if="problem" class="error" role="alert">{{ problem }}</p>

      <label class="field">
        Look inside folders
        <select v-model="mode" :disabled="busy">
          <option v-for="m in IMPORT_MODES" :key="m.value" :value="m.value">{{ m.label }}</option>
        </select>
      </label>
      <p class="hint">{{ modeHelp }}</p>

      <label class="field import-check">
        <input v-model="recursive" type="checkbox" :disabled="busy" />
        Include subfolders
      </label>

      <label class="field">
        Duplicates
        <select v-model="policy" :disabled="busy">
          <option v-for="p in DUPLICATE_POLICIES" :key="p.value" :value="p.value">{{ p.label }}</option>
        </select>
      </label>
      <p class="hint">{{ policyHelp }}</p>

      <div class="import-actions">
        <button type="button" :disabled="busy || !source.trim() || !!problem" @click="start">{{ busy ? "Importing…" : "Import" }}</button>
        <button v-if="busy" type="button" @click="cancel">Stop</button>
        <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      </div>

      <p v-if="progress" class="status" role="status">{{ progress }}</p>
      <p v-if="error" class="error" role="alert">{{ error }}</p>

      <div v-if="report" class="import-report" role="status">
        <p v-for="line in summary" :key="line">{{ line }}</p>
        <details v-if="problems.length">
          <summary>{{ problems.length }} problem{{ problems.length === 1 ? "" : "s" }}</summary>
          <ul class="import-problems">
            <li v-for="p in problems" :key="p">{{ p }}</li>
          </ul>
        </details>
      </div>
    </div>
  </div>
</template>

<style scoped>
.import-panel {
  min-width: min(640px, 92vw);
}
.import-source {
  display: flex;
  gap: 0.35rem;
}
.import-source input {
  flex: 1;
}
.import-check {
  flex-direction: row;
  align-items: center;
  gap: 0.4rem;
}
.import-actions {
  display: flex;
  gap: 0.5rem;
  margin: 0.75rem 0 0.5rem;
}
.import-report p {
  margin: 0.15rem 0;
}
.import-problems {
  max-height: 25vh;
  overflow: auto;
  font-size: var(--fs-small);
  padding-left: 1.2rem;
}
</style>
