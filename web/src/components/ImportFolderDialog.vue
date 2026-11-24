<script setup lang="ts">
// "Add folder": copy a folder tree into the library, as a job with progress.
//
// The folder is chosen with the desktop app's own picker (a path on this
// machine) or typed. Nothing in the source folder is changed: files are
// copied, verified, and laid out as Author/Title (id) or, if asked, in the
// folders they came from. Cancel stops after the
// batch in flight; importing the same folder again carries on where it left
// off. Choices and wording: ../library/importFolder.ts.

import Dialog from "./Dialog.vue";
import { computed, onMounted, ref } from "vue";

import { absorbLibrary, cancelLibraryJob, describeJobProgress, fetchAbsorbable, importFolder } from "../library/api";
import { absorbProblems, describeAbsorb, describeAbsorbable, type AbsorbReport, type AbsorbableLibrary } from "../library/absorb";
import { DEFAULT_IMPORT, DUPLICATE_POLICIES, IMPORT_LAYOUTS, IMPORT_MODES, describeImport, importProblems, validateSource, type DuplicatePolicy, type ImportMode, type ImportReport } from "../library/importFolder";
import { isTauri, tauriInvoke } from "../tauri";

const props = defineProps<{ initialSource?: string }>();
const emit = defineEmits<{ close: []; imported: [] }>();

const source = ref(props.initialSource ?? "");
const mode = ref<ImportMode>(DEFAULT_IMPORT.mode);
const policy = ref<DuplicatePolicy>(DEFAULT_IMPORT.policy);
const recursive = ref(DEFAULT_IMPORT.recursive);
const keepStructure = ref(DEFAULT_IMPORT.keepStructure);
const layoutHelp = computed(() => IMPORT_LAYOUTS.find((l) => l.keep === keepStructure.value)?.help ?? "");

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

// Calibre libraries inside this library: each can be brought in, in place.
// Nothing is moved or copied; see ../library/absorb.ts.
const nested = ref<AbsorbableLibrary[]>([]);
const nestedComplete = ref(true);
const absorbing = ref<string | null>(null);
const absorbJobId = ref<number | null>(null);
const absorbProgress = ref<string | null>(null);
const absorbError = ref<string | null>(null);
const absorbReport = ref<AbsorbReport | null>(null);
const absorbLines = computed(() => (absorbReport.value ? describeAbsorb(absorbReport.value) : []));
const absorbIssues = computed(() => (absorbReport.value ? absorbProblems(absorbReport.value) : []));

async function loadNested() {
  try {
    const found = await fetchAbsorbable();
    nested.value = found.libraries;
    nestedComplete.value = found.conclusive;
  } catch {
    // The list is a convenience; the import above works without it.
    nested.value = [];
  }
}

async function absorb(lib: AbsorbableLibrary) {
  absorbing.value = lib.path;
  absorbError.value = null;
  absorbReport.value = null;
  absorbProgress.value = null;
  try {
    absorbReport.value = await absorbLibrary(lib.path, {
      onStart: (id) => (absorbJobId.value = id),
      onProgress: (job) => (absorbProgress.value = describeJobProgress(job.progress)),
    });
    emit("imported");
  } catch (e) {
    absorbError.value = e instanceof Error ? e.message : String(e);
  } finally {
    absorbing.value = null;
    absorbJobId.value = null;
    absorbProgress.value = null;
    await loadNested();
  }
}

/** Stops after the batch in flight; absorbing the library again carries on. */
async function cancelAbsorb() {
  if (absorbJobId.value === null) return;
  try {
    await cancelLibraryJob(absorbJobId.value);
  } catch (e) {
    absorbError.value = e instanceof Error ? e.message : String(e);
  }
}

onMounted(loadNested);

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
      { source: source.value, mode: mode.value, policy: policy.value, recursive: recursive.value, keep_structure: keepStructure.value },
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
  <Dialog title="Add folder" :busy="busy" size="lg" @close="emit('close')">
    <p class="hint">Copies the books in a folder into this library. The folder you pick is only read: nothing in it is moved or changed.</p>

    <label class="field">
      Folder
      <span class="import-source">
        <input v-model="source" placeholder="Full path of the folder, e.g. D:\Books\Inbox" :disabled="busy" spellcheck="false" />
        <button v-if="isTauri()" type="button" :disabled="busy" @click="browse">Browse…</button>
      </span>
    </label>
    <p v-if="problem" class="error" role="alert">{{ problem }}</p>

    <label class="field">
      Where the copies go
      <select v-model="keepStructure" :disabled="busy" data-testid="layout">
        <option v-for="l in IMPORT_LAYOUTS" :key="String(l.keep)" :value="l.keep">{{ l.label }}</option>
      </select>
    </label>
    <p class="hint" data-testid="layout-help">{{ layoutHelp }}</p>
    <p class="hint">Already have the files where you want them? Open that folder as the library instead, and use Check library to add what is in it, in place.</p>

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

    <p v-if="progress" class="status" role="status">{{ progress }}</p>
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <section v-if="nested.length" class="absorb-section" aria-label="Calibre libraries inside this library">
      <h4>Calibre libraries inside this library</h4>
      <p class="hint">
        Brings a calibre library in where it is: its books, covers and custom columns are registered and no file is moved or copied. Its metadata.db is renamed afterwards (rename it back to undo).
      </p>
      <ul class="absorb-list">
        <li v-for="lib in nested" :key="lib.path">
          <span class="absorb-name">{{ lib.path }}</span>
          <span class="absorb-meta">{{ describeAbsorbable(lib) }}</span>
          <button v-if="lib.state === 'available' && !lib.problem" type="button" :disabled="busy || !!absorbing" @click="absorb(lib)">{{ absorbing === lib.path ? "Absorbing…" : lib.already_absorbed ? "Continue" : "Absorb" }}</button>
          <button v-if="absorbing === lib.path" type="button" @click="cancelAbsorb">Stop</button>
        </li>
      </ul>
      <p v-if="!nestedComplete" class="hint">The search stopped early; more libraries may be in here.</p>
      <p v-if="absorbProgress" class="status" role="status">{{ absorbProgress }}</p>
      <p v-if="absorbError" class="error" role="alert">{{ absorbError }}</p>
      <div v-if="absorbReport" class="import-report" role="status">
        <p v-for="line in absorbLines" :key="line">{{ line }}</p>
        <details v-if="absorbIssues.length">
          <summary>{{ absorbIssues.length }} note{{ absorbIssues.length === 1 ? "" : "s" }}</summary>
          <ul class="import-problems">
            <li v-for="p in absorbIssues" :key="p">{{ p }}</li>
          </ul>
        </details>
      </div>
    </section>

    <div v-if="report" class="import-report" role="status">
      <p v-for="line in summary" :key="line">{{ line }}</p>
      <details v-if="problems.length">
        <summary>{{ problems.length }} problem{{ problems.length === 1 ? "" : "s" }}</summary>
        <ul class="import-problems">
          <li v-for="p in problems" :key="p">{{ p }}</li>
        </ul>
      </details>
    </div>
    <template #footer>
      <button type="button" :disabled="busy" @click="emit('close')">Close</button>
      <button v-if="busy" type="button" @click="cancel">Stop</button>
      <button type="button" class="primary" :disabled="busy || !!absorbing || !source.trim() || !!problem" @click="start">{{ busy ? "Importing…" : "Import" }}</button>
    </template>
  </Dialog>
</template>

<style scoped>
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
.absorb-section {
  margin-top: 1rem;
  border-top: 1px solid var(--border);
  padding-top: 0.5rem;
}
.absorb-list {
  list-style: none;
  padding: 0;
  margin: 0.25rem 0;
}
.absorb-list li {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.15rem 0;
}
.absorb-name {
  flex: 1;
  overflow-wrap: anywhere;
}
.absorb-meta {
  font-size: var(--fs-small);
  opacity: 0.75;
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
