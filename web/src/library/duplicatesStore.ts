// State for the Find duplicates panel: the paged groups, the filters, the
// user's keeper choices, and the "dismissed" list.
//
// The server keeps the last scan in memory and serves it in pages (see
// crates/calibre_srv/src/duplicates_review.rs); this store holds one page at a
// time. Acting on a group (dismiss, merge, delete, resolve) removes it here at
// once and on the server, then re-reads the *same* page offset, so the person
// keeps their place instead of rescanning a 10,000-book library.
//
// The api is injected so the store can be tested without a server.

import { computed, reactive } from "vue";
import * as realApi from "./api";
import { describeJobProgress } from "./api";
import type { DuplicateExemption, DuplicateGroup, DuplicatePage, DuplicateReason, DuplicateScanSummary, DuplicateSort, DuplicatesQuery, EmptyFile, PdfScanStatus } from "./api";

export type DuplicatesApi = Pick<typeof realApi, "fetchDuplicateGroups" | "runDuplicateScan" | "dropDuplicateGroups" | "markNotDuplicates" | "fetchExemptions" | "removeExemption">;

export interface DuplicateFilters {
  /** Empty means every reason. */
  reasons: DuplicateReason[];
  /** 0 to 1. */
  minConfidence: number;
  maxConfidence: number;
  /** Prefix of a file or folder path under the library. */
  path: string;
  sort: DuplicateSort;
  order: "asc" | "desc";
}

export function defaultFilters(): DuplicateFilters {
  return { reasons: [], minConfidence: 0, maxConfidence: 1, path: "", sort: "confidence", order: "desc" };
}

/** The query the server is asked, from the filters and the page. */
export function filtersToQuery(f: DuplicateFilters, offset: number, limit: number): DuplicatesQuery {
  return { offset, limit, reasons: f.reasons, minConfidence: f.minConfidence, maxConfidence: f.maxConfidence, path: f.path, sort: f.sort, order: f.order };
}

/** Whether any filter narrows the list. */
export function filtersActive(f: DuplicateFilters): boolean {
  const d = defaultFilters();
  return f.reasons.length > 0 || f.minConfidence > d.minConfidence || f.maxConfidence < d.maxConfidence || f.path.trim() !== "";
}

export const PAGE_SIZE = 25;

export function createDuplicatesStore(api: DuplicatesApi = realApi, pageSize = PAGE_SIZE) {
  const state = reactive({
    /** The server has a scan to page through. */
    hasScan: false,
    scanning: false,
    scanProgress: "",
    scanJobId: null as number | null,
    loading: false,
    error: null as string | null,
    groups: [] as DuplicateGroup[],
    /** Groups after the filters. */
    total: 0,
    /** Groups in the scan. */
    allTotal: 0,
    offset: 0,
    pageSize,
    facets: {} as Partial<Record<DuplicateReason, number>>,
    /** The library changed since the scan by something other than this panel. */
    stale: false,
    /** An exemption was removed or restored: rescan to see the groups it affects. */
    needsRescan: false,
    scannedAt: null as number | null,
    pdf: undefined as PdfScanStatus | undefined,
    emptyFiles: [] as EmptyFile[],
    exemptionsApplied: 0,
    filters: defaultFilters(),
    /** The copy to keep the user picked, by group id. */
    keepers: {} as Record<string, number>,
    exemptions: [] as DuplicateExemption[],
  });

  const pageNumber = computed(() => Math.floor(state.offset / state.pageSize) + 1);
  const pageCount = computed(() => Math.max(1, Math.ceil(state.total / state.pageSize)));
  const rangeLabel = computed(() => (state.total === 0 ? "0 groups" : `${state.offset + 1}–${state.offset + state.groups.length} of ${state.total.toLocaleString("en-US")}`));

  function apply(page: DuplicatePage) {
    state.hasScan = true;
    state.groups = page.groups;
    state.total = page.total;
    state.allTotal = page.all_total;
    state.offset = page.offset;
    state.facets = page.facets?.reason ?? {};
    state.stale = page.stale;
    state.scannedAt = page.scanned_at;
    state.pdf = page.pdf;
    state.emptyFiles = page.empty_files ?? [];
    state.exemptionsApplied = page.exemptions_applied ?? 0;
  }

  /** Reads the current page from the server. A page past the end (after groups left) steps back. */
  async function load(): Promise<void> {
    state.loading = true;
    state.error = null;
    try {
      let page = await api.fetchDuplicateGroups(filtersToQuery(state.filters, state.offset, state.pageSize));
      if (page.has_scan && page.total > 0 && page.groups.length === 0 && state.offset > 0) {
        const last = Math.floor((page.total - 1) / state.pageSize) * state.pageSize;
        page = await api.fetchDuplicateGroups(filtersToQuery(state.filters, last, state.pageSize));
      }
      if (!page.has_scan) {
        state.hasScan = false;
        state.groups = [];
        state.total = 0;
        state.allTotal = 0;
        return;
      }
      apply(page);
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    } finally {
      state.loading = false;
    }
  }

  /** Scans the whole library as a job with progress, then shows the first page. */
  async function scan(): Promise<DuplicateScanSummary | null> {
    state.scanning = true;
    state.error = null;
    state.needsRescan = false;
    state.scanProgress = "Starting…";
    try {
      const summary = await api.runDuplicateScan("fuzzy", {
        onStart: (id) => (state.scanJobId = id),
        onProgress: (job) => (state.scanProgress = describeJobProgress(job.progress)),
      });
      state.offset = 0;
      await load();
      await loadExemptions();
      return summary;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      return null;
    } finally {
      state.scanning = false;
      state.scanJobId = null;
      state.scanProgress = "";
    }
  }

  /** Shows the server's last scan, or scans if there is none yet. */
  async function open(): Promise<void> {
    await load();
    await loadExemptions();
    if (!state.hasScan && !state.error) await scan();
  }

  async function setFilters(partial: Partial<DuplicateFilters>): Promise<void> {
    state.filters = { ...state.filters, ...partial };
    state.offset = 0;
    await load();
  }

  async function resetFilters(): Promise<void> {
    state.filters = defaultFilters();
    state.offset = 0;
    await load();
  }

  /** Toggles one reason in the filter. */
  async function toggleReason(reason: DuplicateReason): Promise<void> {
    const has = state.filters.reasons.includes(reason);
    await setFilters({ reasons: has ? state.filters.reasons.filter((r) => r !== reason) : [...state.filters.reasons, reason] });
  }

  async function goToPage(n: number): Promise<void> {
    const clamped = Math.min(Math.max(1, n), pageCount.value);
    state.offset = (clamped - 1) * state.pageSize;
    await load();
  }

  /** Takes groups out of the list now, keeping the page and the user's place. */
  function removeLocal(ids: string[]): void {
    const gone = new Set(ids);
    const removed = state.groups.filter((g) => g.id && gone.has(g.id));
    if (removed.length === 0) return;
    state.groups = state.groups.filter((g) => !(g.id && gone.has(g.id)));
    state.total = Math.max(0, state.total - removed.length);
    state.allTotal = Math.max(0, state.allTotal - removed.length);
    for (const g of removed) {
      const n = state.facets[g.reason];
      if (n !== undefined) state.facets[g.reason] = Math.max(0, n - 1);
      if (g.id) delete state.keepers[g.id];
    }
  }

  /** Re-reads the current page quietly so it fills up again from the next groups. */
  async function refill(): Promise<void> {
    try {
      const page = await api.fetchDuplicateGroups(filtersToQuery(state.filters, state.offset, state.pageSize));
      if (page.has_scan) {
        if (page.groups.length === 0 && page.total > 0 && state.offset > 0) {
          state.offset = Math.floor((page.total - 1) / state.pageSize) * state.pageSize;
          await refill();
          return;
        }
        apply(page);
      }
    } catch {
      // The local removal already stands; the next load shows the truth.
    }
  }

  /** Groups dealt with elsewhere (merged, or books deleted): forget them here and on the server. */
  async function resolved(groupIds: string[], deletedBookIds: number[] = []): Promise<void> {
    removeLocal(groupIds);
    try {
      await api.dropDuplicateGroups(groupIds, deletedBookIds);
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
    await refill();
  }

  /** "Not duplicates": every pair in the group is exempt from now on. */
  async function dismiss(group: DuplicateGroup): Promise<boolean> {
    if (!group.id) return false;
    state.error = null;
    try {
      const done = await api.markNotDuplicates({ groupId: group.id });
      removeLocal(done.dropped_group_ids.length > 0 ? done.dropped_group_ids : [group.id]);
      if (done.needs_rescan) state.needsRescan = true;
      state.exemptions = [...state.exemptions, done.exemption];
      await refill();
      return true;
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  async function loadExemptions(): Promise<void> {
    try {
      state.exemptions = (await api.fetchExemptions()).exemptions;
    } catch {
      // The list is a convenience; the scan already applied them.
    }
  }

  /** Undoes a dismissal. The groups it hid come back with a rescan, which this starts. */
  async function undoDismissal(id: string): Promise<void> {
    state.error = null;
    try {
      const done = await api.removeExemption({ id });
      state.exemptions = state.exemptions.filter((e) => e.id !== id);
      if (done.needs_rescan) await scan();
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  async function undoAllDismissals(): Promise<void> {
    state.error = null;
    try {
      const done = await api.removeExemption({ all: true });
      state.exemptions = [];
      if (done.needs_rescan) await scan();
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** The copy to keep: the user's pick, else the server's suggestion. */
  function keeperFor(group: DuplicateGroup): number | null {
    if (group.id && state.keepers[group.id] !== undefined) return state.keepers[group.id];
    return group.suggested_dest ?? null;
  }

  function chooseKeeper(group: DuplicateGroup, bookId: number): void {
    if (!group.id) return;
    if (bookId === group.suggested_dest) delete state.keepers[group.id];
    else state.keepers[group.id] = bookId;
  }

  return { state, pageNumber, pageCount, rangeLabel, load, scan, open, setFilters, resetFilters, toggleReason, goToPage, removeLocal, refill, resolved, dismiss, loadExemptions, undoDismissal, undoAllDismissals, keeperFor, chooseKeeper };
}

export type DuplicatesStore = ReturnType<typeof createDuplicatesStore>;
