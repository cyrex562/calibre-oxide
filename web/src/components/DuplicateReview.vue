<script setup lang="ts">
// The duplicate groups of the last scan, a page at a time, with a side-by-side
// card per group: cover, where each copy lives, how big it is, pages and text
// layer, when it was changed, and the copy the app suggests keeping (and why).
//
// All state is in ../library/duplicatesStore.ts. Acting on a group (not
// duplicates, merge, delete, resolve) takes it out of the list and off the
// server's copy of the scan, and keeps the page where it is.

import { computed, ref } from "vue";

import { keepAndTrashGroup, keepAndTrashLabel, offerRunUndo, partialTrashMessage, trashedCopiesMessage } from "../library/dedupe";
import { trashBooksFlow } from "../library/deleteFlow";
import { confirmDialog } from "./dialogs/confirm";
import type { DuplicateBook, DuplicateGroup, DuplicateReason, DuplicateSort } from "../library/api";
import { ALL_REASONS, confidencePercent, evidenceLines, fileSummary, formatBytes, formatMtime, isImageOnlyScan, needsReview, pdfLine, primaryPath, reasonBadge, reasonLabel, deleteCopyWarnings } from "../library/duplicates";
import { filtersActive } from "../library/duplicatesStore";
import type { DuplicatesStore } from "../library/duplicatesStore";
import { isTauri, tauriInvoke } from "../tauri";

const props = defineProps<{ store: DuplicatesStore }>();
const emit = defineEmits<{ merge: [group: DuplicateGroup]; compare: [group: DuplicateGroup]; details: [bookId: number]; resolve: []; changed: [] }>();

const store = props.store;
const state = store.state;
const desktop = isTauri();

const CONFIDENCE_FROM = [
  { value: 0, label: "any" },
  { value: 0.6, label: "60%" },
  { value: 0.7, label: "70%" },
  { value: 0.8, label: "80%" },
  { value: 0.9, label: "90%" },
  { value: 0.95, label: "95%" },
];
const CONFIDENCE_TO = [
  { value: 1, label: "100%" },
  { value: 0.99, label: "99%" },
  { value: 0.9, label: "90%" },
  { value: 0.85, label: "85%" },
  { value: 0.8, label: "80%" },
  { value: 0.7, label: "70%" },
];
const SORTS: { value: DuplicateSort; label: string }[] = [
  { value: "confidence", label: "Most certain first" },
  { value: "size", label: "Most books first" },
  { value: "reclaimable", label: "Most space first" },
  { value: "reason", label: "Strongest kind first" },
];

const pathDraft = ref(state.filters.path);
const filtered = computed(() => filtersActive(state.filters));
const actionError = ref<string | null>(null);
const showDismissed = ref(false);

/** Reasons the scan found any of, strongest first, with how many groups each. */
const chips = computed(() => ALL_REASONS.filter((r) => (state.facets[r] ?? 0) > 0 || state.filters.reasons.includes(r)));

function chipCount(r: DuplicateReason): number {
  return state.facets[r] ?? 0;
}

function clearFilters() {
  pathDraft.value = "";
  void store.resetFilters();
}

function applyPath() {
  if (pathDraft.value !== state.filters.path) void store.setFilters({ path: pathDraft.value });
}

function isKeeper(group: DuplicateGroup, b: DuplicateBook): boolean {
  return store.keeperFor(group) === b.book_id;
}

function keeperNote(group: DuplicateGroup): string {
  const keeper = group.books.find((b) => b.book_id === store.keeperFor(group));
  if (!keeper) return "";
  if (group.id && state.keepers[group.id] !== undefined) return `Keeping “${keeper.title}”: your choice.`;
  return group.keeper_reason ? `Suggested to keep “${keeper.title}”: ${group.keeper_reason}.` : `Suggested to keep “${keeper.title}”.`;
}

async function dismiss(group: DuplicateGroup) {
  actionError.value = null;
  await store.dismiss(group);
}

function merge(group: DuplicateGroup) {
  emit("merge", group);
}

async function remove(group: DuplicateGroup, book: DuplicateBook) {
  actionError.value = null;
  try {
    const result = await trashBooksFlow([{ id: book.book_id, title: book.title }], {
      warnings: deleteCopyWarnings(group, book, store.keeperFor(group)),
      // Undo puts the copy back in the library; the group is found again by the next scan.
      onRestored: () => emit("changed"),
    });
    if (result.cancelled) return;
    if (result.failureText) actionError.value = result.failureText;
    if (result.outcome?.deleted.includes(book.book_id)) {
      if (group.id) await store.resolved([], [book.book_id]);
      emit("changed");
    }
  } catch (e) {
    actionError.value = e instanceof Error ? e.message : String(e);
  }
}

// Groups a one-click "keep this, trash the others" is running for.
const working = ref<Set<string>>(new Set());

function keeperTitle(group: DuplicateGroup): string {
  return group.books.find((b) => b.book_id === store.keeperFor(group))?.title ?? "";
}

/**
 * One click per group: keep the chosen (or suggested) copy and trash the rest,
 * then the same Undo toast as every other de-dup action. A group that may not be
 * the same book (a title match only) asks first.
 */
async function keepAndTrash(group: DuplicateGroup) {
  const keeper = store.keeperFor(group);
  if (keeper === null || !group.id || working.value.has(group.id)) return;
  actionError.value = null;
  const others = group.books.filter((b) => b.book_id !== keeper);
  if (needsReview(group) && !(await confirmDialog(`These books are grouped on their titles and authors alone, so they may not be the same book. Keep “${keeperTitle(group)}” and move ${others.length === 1 ? "the other one" : `the other ${others.length}`} to the library Trash?`, { title: "Keep and trash", confirmLabel: "Keep and trash", danger: true }))) return;
  working.value = new Set(working.value).add(group.id);
  try {
    const out = await keepAndTrashGroup(group, keeper);
    if (out.partial) {
      // Some copies are already in the Trash: offer the run's Undo, show why the rest stayed, and look again.
      offerRunUndo(partialTrashMessage(out.trashedCount ?? 0, others.length, keeperTitle(group)), out.runId, () => {
        emit("changed");
        void store.scan();
      });
      actionError.value = out.error ?? "Not every other copy could be trashed.";
      emit("changed");
      void store.scan();
      return;
    }
    // Undo brings the copies back, so the group is found again: rescan.
    offerRunUndo(trashedCopiesMessage(out.trashedIds.length, keeperTitle(group)), out.runId, () => {
      emit("changed");
      void store.scan();
    });
    await store.resolved([group.id], out.trashedIds);
    emit("changed");
  } catch (e) {
    actionError.value = e instanceof Error ? e.message : String(e);
  } finally {
    const next = new Set(working.value);
    next.delete(group.id);
    working.value = next;
  }
}

async function openBook(book: DuplicateBook) {
  const fmt = book.files?.[0]?.format;
  if (!fmt) return;
  actionError.value = null;
  try {
    await tauriInvoke<void>("open_book_format", { bookId: book.book_id, fmt });
  } catch (e) {
    actionError.value = e instanceof Error ? e.message : String(e);
  }
}

async function reveal(book: DuplicateBook) {
  actionError.value = null;
  try {
    await tauriInvoke<void>("reveal_book_folder", { bookId: book.book_id });
  } catch (e) {
    actionError.value = e instanceof Error ? e.message : String(e);
  }
}

async function undo(id: string) {
  await store.undoDismissal(id);
}

function dismissedLabel(e: { members: { title: string }[] }): string {
  return e.members.map((m) => m.title || "(untitled)").join("  ≠  ");
}
</script>

<template>
  <div class="dup-review">
    <p v-if="state.scanning" role="status" data-testid="scan-progress">{{ state.scanProgress || "Scanning…" }}</p>
    <p v-if="state.error" class="error" role="alert">{{ state.error }}</p>
    <p v-if="actionError" class="error" role="alert">{{ actionError }}</p>

    <template v-if="!state.scanning">
      <p v-if="state.stale" class="dup-banner" role="status" data-testid="stale-banner">
        The library has changed since this scan, so some groups may be out of date.
        <button type="button" @click="store.scan()">Rescan</button>
      </p>
      <p v-if="state.needsRescan" class="dup-banner" role="status">
        Some groups overlap what you dismissed. <button type="button" @click="store.scan()">Rescan to update them</button>
      </p>

      <div v-if="state.hasScan" class="dup-toolbar">
        <div class="dup-chips" role="group" aria-label="Kind of match">
          <button v-for="r in chips" :key="r" type="button" class="dup-chip" :class="{ on: state.filters.reasons.includes(r) }" :aria-pressed="state.filters.reasons.includes(r)" :data-testid="`chip-${r}`" @click="store.toggleReason(r)">
            {{ reasonLabel(r) }} <span class="count">{{ chipCount(r).toLocaleString("en-US") }}</span>
          </button>
        </div>
        <div class="dup-controls">
          <label>
            Confidence
            <select :value="state.filters.minConfidence" aria-label="Minimum confidence" @change="store.setFilters({ minConfidence: Number(($event.target as HTMLSelectElement).value) })">
              <option v-for="o in CONFIDENCE_FROM" :key="o.value" :value="o.value">{{ o.label }}</option>
            </select>
            to
            <select :value="state.filters.maxConfidence" aria-label="Maximum confidence" @change="store.setFilters({ maxConfidence: Number(($event.target as HTMLSelectElement).value) })">
              <option v-for="o in CONFIDENCE_TO" :key="o.value" :value="o.value">{{ o.label }}</option>
            </select>
          </label>
          <label>
            Folder
            <input v-model="pathDraft" placeholder="starts with…" aria-label="Folder or path prefix" @keydown.enter="applyPath" @blur="applyPath" />
          </label>
          <label>
            Sort
            <select :value="state.filters.sort" aria-label="Sort" @change="store.setFilters({ sort: ($event.target as HTMLSelectElement).value as DuplicateSort })">
              <option v-for="o in SORTS" :key="o.value" :value="o.value">{{ o.label }}</option>
            </select>
          </label>
          <button v-if="filtered" type="button" @click="clearFilters">Clear filters</button>
          <span class="spacer"></span>
          <button type="button" data-testid="open-resolve" @click="emit('resolve')">Resolve…</button>
          <button type="button" @click="store.scan()">Rescan</button>
        </div>
        <p class="hint" data-testid="range">
          {{ store.rangeLabel.value }}<template v-if="filtered"> (of {{ state.allTotal.toLocaleString("en-US") }} groups)</template>
        </p>
      </div>

      <p v-if="state.hasScan && state.allTotal === 0" class="hint">No likely duplicates found.</p>
      <p v-else-if="state.hasScan && state.total === 0" class="hint">No groups match these filters.</p>
      <p v-else-if="!state.hasScan && !state.error && !state.loading" class="hint">No scan yet.</p>

      <article v-for="group in state.groups" :key="group.id ?? group.books.map((b) => b.book_id).join('-')" class="dup-group" data-testid="dup-group">
        <header>
          <span class="dup-badge" :class="`dup-badge-${reasonBadge(group.reason).tone}`" :title="evidenceLines(group).join('\n')" data-testid="dup-badge">
            {{ reasonBadge(group.reason).text }}<template v-if="group.confidence !== undefined"> · {{ confidencePercent(group.confidence) }}</template>
          </span>
          <strong>{{ group.books.length }} books</strong>
          <span class="hint">{{ reasonLabel(group.reason) }}</span>
          <span v-if="group.bytes_reclaimable" class="hint">· {{ formatBytes(group.bytes_reclaimable) }} in the extra copies</span>
          <span v-if="needsReview(group)" class="hint warn">check these are the same book first</span>
          <span class="spacer"></span>
          <button type="button" data-testid="compare" :disabled="group.books.length > 8" :title="group.books.length > 8 ? 'Compare takes up to 8 books at once' : 'Look at these copies side by side: pages, files and metadata'" @click="emit('compare', group)">Compare…</button>
          <button type="button" data-testid="dismiss" title="Never suggest these books as duplicates of each other again" @click="dismiss(group)">Not duplicates</button>
          <button type="button" data-testid="merge" title="Combine these into one record, moving formats and missing details across. This cannot be undone." @click="merge(group)">Merge into…</button>
          <button type="button" class="primary" data-testid="keep-and-trash" :disabled="!group.id || store.keeperFor(group) === null || working.has(group.id)" :title="`Keep “${keeperTitle(group)}” and move the other copies to the library Trash. You can undo it right after.`" @click="keepAndTrash(group)">{{ working.has(group.id ?? "") ? "Trashing…" : keepAndTrashLabel(group) }}</button>
        </header>
        <p v-if="keeperNote(group)" class="hint" data-testid="keeper-note">{{ keeperNote(group) }}</p>

        <div class="dup-compare">
          <section v-for="b in group.books" :key="b.book_id" class="dup-book" :class="{ keeper: isKeeper(group, b) }" data-testid="dup-book">
            <div class="dup-cover">
              <img v-if="b.cover_url" :src="b.cover_url" :alt="`Cover of ${b.title}`" loading="lazy" />
              <span v-else class="no-cover">no cover</span>
            </div>
            <div class="dup-info">
              <button type="button" class="link title" :title="b.title" @click="emit('details', b.book_id)"><span class="clamp">{{ b.title }}</span></button>
              <div class="hint">{{ b.authors.join(", ") }}</div>
              <div v-if="isKeeper(group, b)" class="dup-keep" :title="group.keeper_reason">{{ group.id && state.keepers[group.id] !== undefined ? "Your choice to keep" : "Suggested to keep" }}</div>
              <div class="path" :title="primaryPath(b)" data-testid="dup-path">{{ primaryPath(b) || "—" }}</div>
              <div v-for="f in (b.files ?? []).slice(1)" :key="f.path" class="path secondary" :title="f.path">{{ f.path }}</div>
              <div class="facts" data-testid="dup-facts">
                <span v-if="fileSummary(b)">{{ fileSummary(b) }}</span>
                <span v-if="pdfLine(b)" data-testid="dup-pdf-line">{{ pdfLine(b) }}</span>
                <span v-if="isImageOnlyScan(b)" class="warn">image-only scan</span>
                <span v-if="b.mtime">changed {{ formatMtime(b.mtime) }}</span>
              </div>
              <div class="dup-actions">
                <button v-if="!isKeeper(group, b)" type="button" title="Keep this copy" @click="store.chooseKeeper(group, b.book_id)">Keep this</button>
                <button v-if="desktop && b.files?.length" type="button" @click="openBook(b)">Open</button>
                <button v-if="desktop" type="button" @click="reveal(b)">Reveal</button>
                <button type="button" class="danger" title="Moves the book to the library Trash, from where it can be restored" @click="remove(group, b)">Delete</button>
              </div>
            </div>
          </section>
        </div>
      </article>

      <nav v-if="state.hasScan && store.pageCount.value > 1" class="dup-pager" aria-label="Pages">
        <button type="button" :disabled="state.loading || store.pageNumber.value <= 1" @click="store.goToPage(store.pageNumber.value - 1)">‹ Previous</button>
        <span>Page {{ store.pageNumber.value }} of {{ store.pageCount.value }}</span>
        <button type="button" :disabled="state.loading || store.pageNumber.value >= store.pageCount.value" @click="store.goToPage(store.pageNumber.value + 1)">Next ›</button>
      </nav>

      <details v-if="state.exemptions.length > 0" class="dup-dismissed" :open="showDismissed" data-testid="dismissed" @toggle="showDismissed = ($event.target as HTMLDetailsElement).open">
        <summary>Dismissed ({{ state.exemptions.length.toLocaleString("en-US") }})</summary>
        <p class="hint">Books you said are not duplicates. Undo to see them as a group again.</p>
        <ul>
          <li v-for="e in state.exemptions" :key="e.id">
            <span :title="e.members.map((m) => m.authors.join(', ')).join(' / ')">{{ dismissedLabel(e) }}</span>
            <button type="button" @click="undo(e.id)">Undo</button>
          </li>
        </ul>
        <button v-if="state.exemptions.length > 1" type="button" @click="store.undoAllDismissals()">Undo all</button>
      </details>
    </template>
  </div>
</template>

<style scoped>
.dup-review {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
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
.spacer {
  flex: 1;
}
.dup-banner {
  background: rgba(154, 103, 0, 0.12);
  border: 1px solid var(--warning);
  border-radius: var(--radius-md);
  padding: 0.4rem 0.6rem;
  margin: 0;
}
.dup-toolbar {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  position: sticky;
  top: -1.5em;
  background: var(--bg);
  padding: 0.4rem 0;
  z-index: 1;
}
.dup-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 0.35rem;
}
.dup-chip {
  border: 1px solid var(--border);
  border-radius: var(--radius-pill);
  background: none;
  padding: 0.1rem 0.6rem;
  cursor: pointer;
}
.dup-chip.on {
  background: var(--bg-selected);
  border-color: currentColor;
}
.dup-chip .count {
  color: var(--fg-muted);
  margin-left: 0.2rem;
}
.dup-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 0.6rem;
  align-items: center;
}
.dup-controls label {
  display: inline-flex;
  gap: 0.3rem;
  align-items: center;
}
.dup-group {
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  padding: 0.6rem 0.75rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.dup-group header {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  align-items: center;
}
.dup-compare {
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: minmax(240px, 1fr);
  gap: 0.6rem;
  overflow-x: auto;
}
.dup-book {
  display: flex;
  gap: 0.6rem;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.5rem;
  min-width: 0;
}
.dup-book.keeper {
  border-color: var(--success);
  box-shadow: 0 0 0 1px var(--success) inset;
}
.dup-cover {
  flex: 0 0 64px;
  width: 64px;
  height: 92px;
  background: rgba(127, 127, 127, 0.12);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 0.7em;
  color: var(--fg-muted);
}
.dup-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.dup-info {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}
.dup-info .title {
  font-weight: 600;
  text-align: left;
  /* Not the global fixed button height: a long title takes its own lines. */
  height: auto;
  min-height: 0;
  white-space: normal;
  line-height: 1.3;
}
.dup-info .title .clamp {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  line-clamp: 2;
  overflow: hidden;
  overflow-wrap: anywhere;
}
.dup-info .hint {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.link {
  background: none;
  border: none;
  padding: 0;
  cursor: pointer;
  color: inherit;
  text-decoration: underline;
}
.dup-keep {
  color: var(--success);
  font-size: 0.8em;
  font-weight: 600;
}
.path {
  font-family: var(--font-mono);
  font-size: 0.8em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.path.secondary {
  color: var(--fg-muted);
}
.facts {
  display: flex;
  flex-wrap: wrap;
  gap: 0.2rem 0.6rem;
  font-size: 0.85em;
  color: var(--fg-muted);
}
.dup-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
  margin-top: 0.2rem;
}
.dup-pager {
  display: flex;
  gap: 1rem;
  align-items: center;
  justify-content: center;
}
.dup-dismissed ul {
  list-style: none;
  padding: 0;
  margin: 0.3rem 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}
.dup-dismissed li {
  display: flex;
  gap: 0.6rem;
  align-items: center;
  justify-content: space-between;
}
.dup-badge {
  display: inline-block;
  padding: 0 0.5rem;
  border-radius: var(--radius-pill);
  font-size: 0.75em;
  font-weight: 600;
  border: 1px solid currentColor;
}
.dup-badge-certain {
  color: var(--success);
}
.dup-badge-strong {
  color: var(--accent);
}
.dup-badge-likely {
  color: var(--warning);
}
.dup-badge-review {
  color: var(--warning);
}
</style>
