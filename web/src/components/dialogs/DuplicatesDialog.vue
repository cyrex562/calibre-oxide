<script setup lang="ts">
// Find duplicates: the server scans the whole library once (a job, with
// progress), keeps the result, and the panel pages through it. The state and the
// rules are in ../../library/duplicatesStore.ts and ../../library/resolve.ts.
// It stays mounted while closed, so "Prepare" and "Analyse PDFs" keep their
// progress and Cancel if the dialog is closed and opened again.

import { computed, ref, watch } from "vue";

import { cancelLibraryJob, describeJobProgress, describePrepareProgress, describePrepareResult, fingerprintPdfs, prepareLibrary } from "../../library/api";
import type { DuplicateGroup, EmptyFile, PdfScanStatus } from "../../library/api";
import { fingerprintAction } from "../../library/duplicates";
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

// Identical-file detection compares content hashes, and a library opened
// from real calibre has none. This reads every book file once to record
// one -- a long job on a big library, so it shows progress, can be
// cancelled, and picks up where it stopped when run again.
const prepareBusy = ref(false);
const prepareProgress = ref<string | null>(null);
const prepareSummary = ref<string | null>(null);
const prepareJobId = ref<number | null>(null);

async function prepareForDuplicates() {
  prepareBusy.value = true;
  prepareSummary.value = null;
  prepareProgress.value = null;
  try {
    // One job: scan, hash, PDF fingerprints, duplicate scan. Its stage
    // is kept on the server, so running it again resumes where it stopped.
    let cancelled = false;
    const result = await prepareLibrary({
      onStart: (id) => (prepareJobId.value = id),
      onProgress: (job) => {
        prepareProgress.value = describePrepareProgress(job.progress);
        cancelled = job.progress.cancelled;
      },
    });
    prepareSummary.value = describePrepareResult(result, cancelled);
    // The chain's last stage already scanned for duplicates; load the groups.
    await props.store.scan();
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  } finally {
    prepareBusy.value = false;
    prepareProgress.value = null;
    prepareJobId.value = null;
  }
}

async function cancelPrepare() {
  if (prepareJobId.value === null) return;
  try {
    await cancelLibraryJob(prepareJobId.value);
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  }
}

// Scan-vs-OCR and optimizer re-saves are found from the pages themselves,
// which means rendering a few pages of every PDF once. A long job on a big
// library, so: progress, Cancel, and it resumes where it stopped.
const fingerprintBusy = ref(false);
const fingerprintProgress = ref<string | null>(null);
const fingerprintSummary = ref<string | null>(null);
const fingerprintJobId = ref<number | null>(null);

async function runFingerprint() {
  fingerprintBusy.value = true;
  fingerprintSummary.value = null;
  fingerprintProgress.value = null;
  try {
    const result = await fingerprintPdfs({
      onStart: (id) => (fingerprintJobId.value = id),
      onProgress: (job) => (fingerprintProgress.value = describeJobProgress(job.progress)),
    });
    const parts: string[] = [];
    if (result.cancelled) parts.push("stopped early — run it again to continue");
    parts.push(`${result.fingerprinted.toLocaleString("en-US")} of ${result.total.toLocaleString("en-US")} PDFs analysed`);
    if (result.failed_count > 0) parts.push(`${result.failed_count} could not be read (encrypted or damaged)`);
    if (result.missing_count > 0) parts.push(`${result.missing_count} listed but not found on disk`);
    fingerprintSummary.value = parts.join(" · ");
    await props.store.scan();
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  } finally {
    fingerprintBusy.value = false;
    fingerprintProgress.value = null;
    fingerprintJobId.value = null;
  }
}

async function cancelFingerprint() {
  if (fingerprintJobId.value === null) return;
  try {
    await cancelLibraryJob(fingerprintJobId.value);
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
    <Dialog :open="open" title="Find duplicates" show-close size="full" @close="emit('close')">
      <section>
        <!--
          Identical files are found by content hash, which only exist
          for books this app added or scanned. A library opened from real
          calibre needs them recorded first.
        -->
        <div class="plugin-actions">
          <button type="button" :disabled="prepareBusy" @click="prepareForDuplicates">
            {{ prepareBusy ? "Reading files…" : "Prepare for duplicate detection" }}
          </button>
          <button v-if="prepareBusy" type="button" @click="cancelPrepare">Cancel</button>
        </div>
        <p v-if="prepareProgress" class="news-hint" role="status">{{ prepareProgress }}</p>
        <p v-if="prepareSummary" class="news-hint">{{ prepareSummary }}</p>
        <!--
          A scan and its OCR copy, or a PDF and its optimized copy, share
          no bytes and often no metadata; they are found from their pages,
          which have to be analysed once.
        -->
        <div v-if="pdfAction || fingerprintBusy" class="plugin-actions">
          <button type="button" data-testid="fingerprint-pdfs" :disabled="fingerprintBusy" @click="runFingerprint">
            {{ fingerprintBusy ? "Analysing PDFs…" : pdfAction?.label }}
          </button>
          <button v-if="fingerprintBusy" type="button" @click="cancelFingerprint">Cancel</button>
        </div>
        <p v-if="pdfAction?.note" class="news-hint">{{ pdfAction.note }}</p>
        <p v-if="fingerprintProgress" class="news-hint" role="status">{{ fingerprintProgress }}</p>
        <p v-if="fingerprintSummary" class="news-hint">{{ fingerprintSummary }}</p>
        <p v-if="emptyFiles.length" class="news-hint" data-testid="empty-files">
          {{ emptyFiles.length }} empty (0-byte) file{{ emptyFiles.length === 1 ? "" : "s" }}, not counted as duplicates:
          {{ emptyFiles.slice(0, 5).map((f) => `${f.title} (.${f.format.toLowerCase()})`).join(", ") }}{{ emptyFiles.length > 5 ? ", …" : "" }}
        </p>
        <p v-if="duplicatesError" class="error">{{ duplicatesError }}</p>
        <p v-if="mergeNote" class="news-hint" role="status">{{ mergeNote }}</p>
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
