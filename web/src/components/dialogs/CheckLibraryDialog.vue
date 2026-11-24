<script setup lang="ts">
// Check library (issue #748) -- a real integrity scan, see
// crates/lexmechanic_srv/src/check_library.rs's own doc. It stays mounted while
// closed so a scan the user started keeps its progress and Cancel if the dialog
// is closed and opened again; opening it runs the check afresh.

import { computed, ref, watch } from "vue";

import {
  CHECK_LIBRARY_LABELS, cancelLibraryJob, checkLibrary, describeJobProgress, fetchIgnored, fetchOrphans, forgetOrphan, relocateOrphan, scanLibrary, unignoreFile, uploadOrphanFile,
} from "../../library/api";
import type { CheckLibraryFinding, CheckLibraryResult, IgnoredFile, Orphan } from "../../library/api";
import { findingName } from "../../library/checkLibraryView";
import Dialog from "../Dialog.vue";
import LocateMissing from "./LocateMissing.vue";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{
  close: [];
  /** Books were added or changed: the list is stale. */
  changed: [];
  /** A finding names a book: close and open it in the details panel. */
  "show-book": [bookId: number];
}>();

const checkLibraryLoading = ref(false);
const checkLibraryError = ref<string | null>(null);
const checkLibraryResult = ref<CheckLibraryResult | null>(null);

/**
 * The non-empty categories, in the order `CHECK_LIBRARY_LABELS` lists
 * them — most worth reading first. `conclusive`/`needs_attention` are
 * booleans in the same object and are not categories.
 */
const checkLibraryNonEmpty = computed(() => {
  const result = checkLibraryResult.value;
  if (!result) return [];
  return Object.keys(CHECK_LIBRARY_LABELS)
    .map((key) => [key, result[key]] as const)
    .filter((entry): entry is readonly [string, CheckLibraryFinding[]] => Array.isArray(entry[1]) && entry[1].length > 0);
});

/**
 * False when a folder could not be read. Saying so matters: the check
 * deliberately computes no missing-file list in that case, and a silent
 * empty list would read as "nothing is missing".
 */
const checkLibraryConclusive = computed(() => checkLibraryResult.value?.conclusive !== false);

const rescanBusy = ref(false);
const rescanSummary = ref<string | null>(null);
/** Live progress line while the scan job runs, and the id Cancel acts on. */
const rescanProgress = ref<string | null>(null);
const rescanJobId = ref<number | null>(null);

/** Stops the running scan. It keeps what it has added; scanning again continues. */
async function cancelRescan() {
  if (rescanJobId.value === null) return;
  try {
    await cancelLibraryJob(rescanJobId.value);
  } catch (e) {
    checkLibraryError.value = e instanceof Error ? e.message : String(e);
  }
}

/**
 * Untracked files are files the user can see in their own folder that
 * the library does not list — so the check reports them and this puts
 * them in. Offered from the check dialog rather than as its own menu
 * item because that is where the user has just been told they exist.
 *
 * A scan also re-attaches anything that moved, so the check is re-run
 * afterwards instead of assuming what changed.
 */
async function rescanLibraryFolder() {
  rescanBusy.value = true;
  rescanSummary.value = null;
  rescanProgress.value = null;
  checkLibraryError.value = null;
  try {
    // A scan of a big folder runs for a long time, so it is a background
    // job: show how far it has got, and let the user stop it.
    const result = await scanLibrary({
      onStart: (id) => (rescanJobId.value = id),
      onProgress: (job) => (rescanProgress.value = describeJobProgress(job.progress)),
    });
    const parts: string[] = [];
    if (result.cancelled) parts.push("stopped early — scan again to continue where it left off");
    if (result.added.length > 0) parts.push(`${result.added.length} added`);
    if (result.relocated > 0) parts.push(`${result.relocated} found in a new place`);
    // Named rather than counted: a file the scan refused to add is
    // exactly what the user needs to know the name of.
    if (result.settling.length > 0) parts.push(`still being written: ${result.settling.join(", ")}`);
    if (result.ignored.length > 0) parts.push(`kept out because you removed them: ${result.ignored.join(", ")}`);
    for (const failure of result.failed) parts.push(`${failure.path}: ${failure.error}`);
    if (!result.conclusive) parts.push("a folder could not be read, so this is partial");
    rescanSummary.value = parts.length > 0 ? parts.join(" · ") : "Nothing to add.";
    emit("changed");
    checkLibraryResult.value = await checkLibrary();
  } catch (e) {
    checkLibraryError.value = e instanceof Error ? e.message : String(e);
  } finally {
    rescanBusy.value = false;
    rescanProgress.value = null;
    rescanJobId.value = null;
  }
}

/** Whether the check found files on disk that no book claims. */
const checkLibraryHasUntracked = computed(() => {
  const untracked = checkLibraryResult.value?.untracked_files;
  return Array.isArray(untracked) && untracked.length > 0;
});

/*
 * Orphans and the ignore list (#921).
 *
 * Both are rendered from their own endpoints rather than from the
 * check's findings, because the check reports only `{book_id, title,
 * path}` and resolving an orphan needs its *format* — `relocateOrphan`
 * points at one `(book, format)` pair, not at a book.
 *
 * They live in this dialog because it is where the user has just been
 * told they exist. An orphan is the one state the tracked model requires
 * a person to act on, so reporting it without the three resolutions
 * would be telling somebody their library is broken and offering
 * nothing.
 */
const orphanList = ref<Orphan[]>([]);
const ignoredList = ref<IgnoredFile[]>([]);
const orphanBusy = ref<string | null>(null);
const orphanMessage = ref<string | null>(null);

/** Keys rendered by their own sections below, so the generic list skips them. */
const SELF_RENDERED_CHECK_KEYS = new Set(["orphaned_books", "ignored_files"]);

const checkLibraryGenericFindings = computed(() => checkLibraryNonEmpty.value.filter(([key]) => !SELF_RENDERED_CHECK_KEYS.has(key)));

/** The untracked files, offered as relocation candidates for an orphan. */
const untrackedPaths = computed(() => {
  const untracked = checkLibraryResult.value?.untracked_files;
  return Array.isArray(untracked) ? untracked.map((f) => f.path) : [];
});

function orphanKey(orphan: Orphan) {
  return `${orphan.book_id}:${orphan.format}`;
}

async function loadOrphanState() {
  try {
    [orphanList.value, ignoredList.value] = await Promise.all([fetchOrphans(), fetchIgnored()]);
  } catch (e) {
    checkLibraryError.value = e instanceof Error ? e.message : String(e);
  }
}

/**
 * Runs one resolution, then reloads. Every one of them changes both the
 * orphan list and the check's own findings, so nothing here assumes what
 * changed.
 */
async function resolveOrphan(orphan: Orphan, action: () => Promise<string | null>) {
  orphanBusy.value = orphanKey(orphan);
  orphanMessage.value = null;
  try {
    orphanMessage.value = await action();
    await loadOrphanState();
    emit("changed");
    checkLibraryResult.value = await checkLibrary();
  } catch (e) {
    orphanMessage.value = e instanceof Error ? e.message : String(e);
  } finally {
    orphanBusy.value = null;
  }
}

function relocateTo(orphan: Orphan, path: string) {
  if (!path) return;
  // The verdict is the point of this call, not a detail: the user may
  // have picked a re-downloaded copy, or the wrong file entirely.
  void resolveOrphan(orphan, async () => (await relocateOrphan(orphan.book_id, orphan.format, path)).message);
}

function uploadFor(orphan: Orphan, event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) return;
  void resolveOrphan(orphan, async () => (await uploadOrphanFile(orphan.book_id, orphan.format, file)).message);
}

function forgetOrphanEntry(orphan: Orphan, keepMetadata: boolean) {
  void resolveOrphan(orphan, async () => {
    // Named after the file that was lost, so the saved metadata is
    // findable later without guessing.
    const name = keepMetadata ? `${orphan.last_known_path.split("/").pop() ?? "book"}.opf` : undefined;
    const result = await forgetOrphan(orphan.book_id, name);
    return result.metadata_saved_as ? `Entry removed. Metadata saved as ${result.metadata_saved_as}.` : "Entry removed.";
  });
}

async function unignore(file: IgnoredFile) {
  orphanBusy.value = file.path;
  orphanMessage.value = null;
  try {
    await unignoreFile(file.path);
    // Deliberately not scanned here: unignoring only lifts the refusal.
    // The next scan adds the file, and saying so beats appearing to do
    // nothing.
    orphanMessage.value = `${file.path} will be picked up by the next scan.`;
    await loadOrphanState();
  } catch (e) {
    orphanMessage.value = e instanceof Error ? e.message : String(e);
  } finally {
    orphanBusy.value = null;
  }
}

async function runCheck() {
  checkLibraryError.value = null;
  checkLibraryLoading.value = true;
  checkLibraryResult.value = null;
  try {
    checkLibraryResult.value = await checkLibrary();
    await loadOrphanState();
  } catch (e) {
    checkLibraryError.value = e instanceof Error ? e.message : String(e);
  } finally {
    checkLibraryLoading.value = false;
  }
}
watch(
  () => props.open,
  (open) => {
    if (open) void runCheck();
  },
  { immediate: true },
);
</script>

<template>
    <Dialog :open="open" title="Check library" show-close size="lg" @close="emit('close')">
      <section>
        <p v-if="checkLibraryLoading">Scanning…</p>
        <p v-else-if="checkLibraryError" class="error">{{ checkLibraryError }}</p>
        <template v-else-if="checkLibraryResult">
          <!--
            An incomplete scan computes no missing-file list at all, so
            saying "no problems found" would be a lie. Told first,
            because it changes how everything below should be read.
          -->
          <p v-if="!checkLibraryConclusive" class="error">
            A folder could not be read, so this check is incomplete — nothing is reported as missing, because a file that cannot be seen cannot be told from one that is gone.
          </p>
          <p v-if="checkLibraryNonEmpty.length === 0" class="news-hint">{{ checkLibraryConclusive ? "No problems found." : "Nothing else to report." }}</p>
          <!--
            Only when there is something to add. A button that does
            nothing is worse than no button.
          -->
          <div v-if="checkLibraryHasUntracked" class="plugin-actions">
            <button type="button" :disabled="rescanBusy" @click="rescanLibraryFolder">
              {{ rescanBusy ? "Scanning…" : "Add the untracked files to the library" }}
            </button>
            <button v-if="rescanBusy" type="button" @click="cancelRescan">Cancel</button>
          </div>
          <p v-if="rescanProgress" class="news-hint" role="status">{{ rescanProgress }}</p>
          <p v-if="rescanSummary" class="news-hint">{{ rescanSummary }}</p>
          <div v-for="[key, findings] in checkLibraryGenericFindings" :key="key">
            <h4>{{ CHECK_LIBRARY_LABELS[key] ?? key }} ({{ findings.length }})</h4>
            <ul class="manage-list">
              <li v-for="(f, i) in findings" :key="i">
                <span class="manage-name">{{ findingName(f) }}</span>
                <code class="manage-query" :title="f.path">{{ f.path }}</code>
                <button v-if="f.book_id !== null" type="button" data-action="show-book" @click="emit('show-book', f.book_id)">Show book</button>
              </li>
            </ul>
          </div>

          <!--
            An orphan is the one state this model requires a person to
            act on, so it gets the three resolutions the design
            promises rather than a line of text.
          -->
          <div v-if="orphanList.length > 0">
            <h4>Books whose file is gone ({{ orphanList.length }})</h4>
            <ul class="manage-list">
              <li v-for="orphan in orphanList" :key="orphanKey(orphan)" class="orphan-row">
                <div>
                  <span class="manage-name">{{ orphan.format }}</span>
                  <code class="manage-query">{{ orphan.last_known_path }}</code>
                </div>
                <div class="plugin-actions">
                  <!--
                    Only offered when there is something to point at.
                    The candidates are the files the check just found
                    that no book claims.
                  -->
                  <label v-if="untrackedPaths.length > 0">
                    Point at:
                    <select :disabled="orphanBusy === orphanKey(orphan)" @change="relocateTo(orphan, ($event.target as HTMLSelectElement).value)">
                      <option value="">Choose a file…</option>
                      <option v-for="path in untrackedPaths" :key="path" :value="path">{{ path }}</option>
                    </select>
                  </label>
                  <LocateMissing :book-id="orphan.book_id" :format="orphan.format" :disabled="orphanBusy === orphanKey(orphan)" @use="(path) => relocateTo(orphan, path)" />
                  <label class="orphan-upload">
                    Copy a file back in…
                    <input type="file" :disabled="orphanBusy === orphanKey(orphan)" @change="uploadFor(orphan, $event)" />
                  </label>
                  <button type="button" :disabled="orphanBusy === orphanKey(orphan)" @click="forgetOrphanEntry(orphan, true)">Remove entry, keep metadata</button>
                  <button type="button" :disabled="orphanBusy === orphanKey(orphan)" @click="forgetOrphanEntry(orphan, false)">Remove entry</button>
                </div>
              </li>
            </ul>
          </div>

          <!--
            Visible and undoable: a hidden list of files the app
            refuses to show is its own kind of bug.
          -->
          <div v-if="ignoredList.length > 0">
            <h4>Removed from the library, kept on disk ({{ ignoredList.length }})</h4>
            <ul class="manage-list">
              <li v-for="file in ignoredList" :key="file.path">
                <span class="manage-name">{{ file.title || "—" }}</span>
                <code class="manage-query">{{ file.path }}</code>
                <button type="button" :disabled="orphanBusy === file.path" @click="unignore(file)">Add it back</button>
              </li>
            </ul>
          </div>

          <p v-if="orphanMessage" class="news-hint">{{ orphanMessage }}</p>
        </template>
      </section>
      <template #footer>
        <button type="button" data-action="recheck" :disabled="checkLibraryLoading || rescanBusy" @click="runCheck">{{ checkLibraryLoading ? "Checking…" : "Check again" }}</button>
        <button type="button" class="primary" @click="emit('close')">Close</button>
      </template>
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
/*
 * An orphan row carries four controls, which do not fit on the same line
 * as the path on a narrow panel -- so it stacks rather than overflowing.
 */
.orphan-row {
  flex-direction: column;
  align-items: flex-start;
  gap: 0.35em;
}

/* The file input is wide and ugly bare; the label carries the wording. */
.orphan-upload {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
}

.orphan-upload input[type="file"] {
  max-width: 14em;
}
</style>
