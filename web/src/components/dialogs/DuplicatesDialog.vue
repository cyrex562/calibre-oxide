<script setup lang="ts">
// Find duplicates: the server scans the whole library once (a job, with
// progress), keeps the result, and the panel pages through it. The state and the
// rules are in ../../library/duplicatesStore.ts and ../../library/resolve.ts.
// It stays mounted while closed, so "Analyse library" keeps its progress and
// Cancel if the dialog is closed and opened again.

import { computed, ref, watch } from "vue";

import { cancelLibraryJob, describePrepareProgress, describePrepareResult, prepareLibrary } from "../../library/api";
import type { DuplicateGroup, EmptyFile, PdfScanStatus } from "../../library/api";
import { analyseLabelFor, fingerprintAction, pageMatchNoteFor } from "../../library/duplicates";
import { filtersActive } from "../../library/duplicatesStore";
import type { DuplicatesStore } from "../../library/duplicatesStore";
import Dialog from "../Dialog.vue";
import DuplicateReview from "../DuplicateReview.vue";

const props = defineProps<{
  open: boolean;
  store: DuplicatesStore;
  /** What the last merge did, shown above the groups. */
  mergeNote: string | null;
}>();
const emit = defineEmits<{
  close: [];
  merge: [group: DuplicateGroup];
  compare: [group: DuplicateGroup];
  details: [bookId: number];
  resolve: [];
  /** Books changed under the panel (a delete, a resolve run): the list is stale. */
  changed: [];
}>();

const duplicatesError = ref<string | null>(null);
watch(
  () => props.open,
  (open) => {
    if (open) duplicatesError.value = null;
  },
);

// How many PDFs are not yet fingerprinted, for page-content matching.
const pdfStatus = computed<PdfScanStatus | undefined>(() => props.store.state.pdf);
// Zero-byte files: not duplicates of each other, listed separately.
const emptyFiles = computed<EmptyFile[]>(() => props.store.state.emptyFiles);
const pdfAction = computed(() => fingerprintAction(pdfStatus.value));

// One button does all the preparation. Identical-file detection compares
// content hashes (a library opened from real calibre has none), and a PDF and
// its re-saved or OCR copy are found from their pages, which have to be
// analysed once. The server runs it as one job -- read the files, analyse the
// PDFs, scan for duplicates -- that shows progress, can be cancelled, and picks
// up where it stopped when run again. The groups are loaded when it finishes.
const analyseBusy = ref(false);
const analyseProgress = ref<string | null>(null);
const analyseJobId = ref<number | null>(null);
// What the last job (or the last merge) did. One line, replaced each time
// rather than stacked under the earlier ones.
const statusLine = ref<string | null>(null);
watch(
  () => props.mergeNote,
  (note) => {
    if (note) statusLine.value = note;
  },
  { immediate: true },
);

const analyseLabel = computed(() => (analyseBusy.value ? "Analysing…" : analyseLabelFor(pdfStatus.value)));

async function analyseLibrary() {
  analyseBusy.value = true;
  statusLine.value = null;
  analyseProgress.value = null;
  duplicatesError.value = null;
  try {
    let cancelled = false;
    const result = await prepareLibrary({
      onStart: (id) => (analyseJobId.value = id),
      onProgress: (job) => {
        analyseProgress.value = describePrepareProgress(job.progress);
        cancelled = job.progress.cancelled;
      },
    });
    statusLine.value = describePrepareResult(result, cancelled);
    // The chain's last stage already scanned for duplicates; load the groups.
    await props.store.scan();
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  } finally {
    analyseBusy.value = false;
    analyseProgress.value = null;
    analyseJobId.value = null;
  }
}

async function cancelAnalyse() {
  if (analyseJobId.value === null) return;
  try {
    await cancelLibraryJob(analyseJobId.value);
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  }
}

// Said above the groups when the page analysis ran and found nothing, so an
// empty "same PDF, re-saved" list is not mistaken for a skipped step.
const pageMatchNote = computed(() => pageMatchNoteFor(pdfStatus.value, props.store.state.facets, filtersActive(props.store.state.filters)));
</script>

<template>
    <Dialog :open="open" title="Find duplicates" show-close size="full" @close="emit('close')">
      <section>
        <div class="plugin-actions">
          <button type="button" data-testid="analyse-library" :disabled="analyseBusy" title="Reads every book file once, looks at the pages of every PDF, then looks for duplicates. It can be stopped and picks up where it stopped." @click="analyseLibrary">{{ analyseLabel }}</button>
          <button v-if="analyseBusy" type="button" @click="cancelAnalyse">Cancel</button>
        </div>
        <p v-if="pdfAction?.note && !analyseBusy" class="news-hint">{{ pdfAction.note }}</p>
        <p v-if="analyseProgress" class="news-hint" role="status">{{ analyseProgress }}</p>
        <p v-if="statusLine" class="news-hint" role="status" data-testid="status-line">{{ statusLine }}</p>
        <p v-if="pageMatchNote" class="news-hint" data-testid="page-match-note">{{ pageMatchNote }}</p>
        <p v-if="emptyFiles.length" class="news-hint" data-testid="empty-files">
          {{ emptyFiles.length }} empty (0-byte) file{{ emptyFiles.length === 1 ? "" : "s" }}, not counted as duplicates:
          {{ emptyFiles.slice(0, 5).map((f) => `${f.title} (.${f.format.toLowerCase()})`).join(", ") }}{{ emptyFiles.length > 5 ? ", …" : "" }}
        </p>
        <p v-if="duplicatesError" class="error">{{ duplicatesError }}</p>
        <DuplicateReview :store="store" @merge="emit('merge', $event)" @compare="emit('compare', $event)" @details="(id) => emit('details', id)" @resolve="emit('resolve')" @changed="emit('changed')" />
      </section>
    </Dialog>
</template>

<style scoped>
.error {
  padding: 1em;
  color: var(--danger);
}
</style>
