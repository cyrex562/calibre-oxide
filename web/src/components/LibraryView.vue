<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import CategoryBrowser from "./CategoryBrowser.vue";
import NoteEditor from "./NoteEditor.vue";
import BookDetailsPanel from "./BookDetailsPanel.vue";
import BookTable from "./BookTable.vue";
import ContextMenu from "./ContextMenu.vue";
import { addBook, addCustomColumn, addNewsSchedule, catalogDownloadUrl, CHECK_LIBRARY_LABELS, checkLibrary, deleteBooks, deleteSavedSearch, deleteVirtualLibrary, fetchBooks, fetchCustomColumns, fetchFieldMetadata, fetchSavedSearches, fetchVirtualLibraries, ftsSearch, ftsSnippets, getNewsFetchStatus, importOpml, libraryExportUrl, listNewsSchedules, removeCustomColumn, removeNewsSchedule, renameSavedSearch, runNewsScheduleNow, saveToDisk, scanForDuplicates, search, setFields, setFtsEnabled, setSavedSearch, startNewsFetch, setVirtualLibrary } from "../library/api";
import type { CheckLibraryResult, CustomRecipeOptions, DuplicateBook, NewsFeedInput, NewsSchedule, SaveToDiskResult } from "../library/api";
import { parseSnippetSegments } from "../library/snippets";
import { clampWidth, columnsFor, DEFAULT_TABLE_PREFS, resolveColumns, TABLE_PREFS_PROFILE, type BookColumn, type LibraryViewMode, type TablePrefs } from "../library/columns";
import { actionEnabled, contextMenuEntries, LIBRARY_ACTIONS, visibleToolbarActions, type ActionContext, type LibraryAction, type LibraryActionId } from "../library/actions";
import { onMenuAction, syncDesktopMenu } from "../library/desktopMenu";
import { shortcutFor } from "../library/shortcuts";
import { isTauri, tauriInvoke } from "../tauri";
import { DEFAULT_KEYMAP, DEFAULT_LIBRARY_PREFS, DEFAULT_TOOLBAR_PREFS, fetchProfile, KEYMAP_PROFILE, libraryShortcuts, LIBRARY_PREFS_PROFILE, saveProfile, TOOLBAR_PREFS_PROFILE, type KeymapPrefs, type LibraryPrefs, type ToolbarActionId, type ToolbarPrefs } from "../settings/api";
import type { BookFieldChanges, BookSummary, CustomColumnInfo, FieldMetaEntry, FtsSnippet } from "../library/types";

// Real, persisted default (issue #721) -- overwritten by
// loadLibraryPrefs() below once its fetch resolves; starts at the
// same value DEFAULT_LIBRARY_PREFS uses so there's no visible flash
// for a user with no saved preferences yet.
const pageSize = ref(DEFAULT_LIBRARY_PREFS.pageSize);
const duplicateDefault = ref<LibraryPrefs["duplicateDefault"]>(DEFAULT_LIBRARY_PREFS.duplicateDefault);

const queryText = ref("");
const activeQuery = ref(""); // committed query -- what's actually sent, vs. the input box's live text
const sort = ref(DEFAULT_LIBRARY_PREFS.sort);
const sortOrder = ref<"asc" | "desc">(DEFAULT_LIBRARY_PREFS.sortOrder);
const vl = ref("");
const offset = ref(0);

const sortableFields = ref<[string, string][]>([]);
const fieldMetadata = ref<Record<string, FieldMetaEntry>>({});
const virtualLibraries = ref<Record<string, string>>({});
const savedSearches = ref<Record<string, string>>({});

// Multi-field sort (issue #759) -- `sort` itself stays a single
// comma-joined string (matches /ajax/search's own real request shape
// and this port's existing library-prefs storage, which already
// persists `sort` as a plain string -- no format change needed there).
// This is just an ordered-list view/editor over that same string.
const sortFields = computed(() => sort.value.split(",").map((s) => s.trim()).filter(Boolean));
const primarySort = computed({
  get: () => sortFields.value[0] ?? "timestamp",
  set: (field: string) => {
    sort.value = [field, ...sortFields.value.slice(1)].join(",");
  },
});
const secondarySortFields = computed(() => sortFields.value.slice(1));
const availableExtraSortFields = computed(() => sortableFields.value.filter(([key]) => !sortFields.value.includes(key)));

function fieldLabel(key: string): string {
  return sortableFields.value.find(([k]) => k === key)?.[1] ?? key;
}

function addSortField(field: string) {
  if (!field || sortFields.value.includes(field)) return;
  sort.value = [...sortFields.value, field].join(",");
}

function removeSortField(field: string) {
  sort.value = sortFields.value.filter((f) => f !== field).join(",");
}

const books = ref<BookSummary[]>([]);
const totalNum = ref(0);
const loading = ref(false);
const error = ref<string | null>(null);
const selectedBookId = ref<number | null>(null);
const cacheBust = ref(0);

const addInput = ref<HTMLInputElement | null>(null);
const adding = ref(false);
const addError = ref<string | null>(null);

const pageCount = computed(() => Math.max(1, Math.ceil(totalNum.value / pageSize.value)));
const currentPage = computed(() => Math.floor(offset.value / pageSize.value) + 1);

async function loadMetadata() {
  try {
    const [fm, vls, searches] = await Promise.all([fetchFieldMetadata(), fetchVirtualLibraries(), fetchSavedSearches()]);
    sortableFields.value = fm.sortable_fields;
    fieldMetadata.value = fm.field_metadata;
    virtualLibraries.value = vls;
    savedSearches.value = searches;
  } catch (e) {
    // Non-fatal -- the grid itself still works with default sort/no vl.
    console.error("failed to load field metadata / virtual libraries / saved searches", e);
  }
}
void loadMetadata();

async function loadLibraryPrefs() {
  try {
    const prefs = await fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE);
    if (!prefs) return;
    sort.value = prefs.sort;
    sortOrder.value = prefs.sortOrder;
    pageSize.value = prefs.pageSize;
    duplicateDefault.value = prefs.duplicateDefault;
  } catch (e) {
    // Non-fatal -- the grid still works with the built-in defaults.
    console.error("failed to load library preferences", e);
  }
}
void loadLibraryPrefs();

// Real toolbar customization (#753) -- see settings/api.ts's own doc
// for the action-registry design. Loaded once on mount, same pattern
// as library prefs above; a settings-panel edit re-loads it here too
// so a change is visible without a full page reload (the issue's own
// definition of done only requires surviving a real reload, but this
// is free given the existing fetchProfile-on-mount shape).
const toolbarPrefs = ref<ToolbarPrefs>({ ...DEFAULT_TOOLBAR_PREFS });

async function loadToolbarPrefs() {
  try {
    const prefs = await fetchProfile<ToolbarPrefs>(TOOLBAR_PREFS_PROFILE);
    if (prefs) toolbarPrefs.value = { ...DEFAULT_TOOLBAR_PREFS, ...prefs };
  } catch (e) {
    console.error("failed to load toolbar preferences", e);
  }
}
void loadToolbarPrefs();

// ---------------------------------------------------------------
// Table view (issue 1.1)
// ---------------------------------------------------------------
//
// The table is the default view: this tool is aimed at PDF
// collections, where covers are usually blank or identical and the
// cover grid degrades into a wall of grey rectangles. The grid stays
// one click away for libraries where covers do carry information.
//
// Column *decisions* -- what a cell shows, which columns exist, what a
// header click means -- all live in library/columns.ts as pure
// functions, so they are tested directly rather than through a mount.

const tablePrefs = ref<TablePrefs>({ ...DEFAULT_TABLE_PREFS });

async function loadTablePrefs() {
  try {
    const prefs = await fetchProfile<TablePrefs>(TABLE_PREFS_PROFILE);
    if (prefs) tablePrefs.value = { ...DEFAULT_TABLE_PREFS, ...prefs };
  } catch (e) {
    console.error("failed to load table preferences", e);
  }
}
void loadTablePrefs();

async function saveTablePrefs() {
  try {
    await saveProfile(TABLE_PREFS_PROFILE, { ...tablePrefs.value });
  } catch (e) {
    console.error("failed to save table preferences", e);
  }
}

const viewMode = computed<LibraryViewMode>(() => tablePrefs.value.view);

/** Every column this library could show, including its custom ones. */
const availableColumns = computed<BookColumn[]>(() => columnsFor(fieldMetadata.value, sortableFields.value));

/** The columns actually rendered, with saved widths applied. */
const visibleColumns = computed<BookColumn[]>(() => resolveColumns(availableColumns.value, tablePrefs.value));

function setViewMode(mode: LibraryViewMode) {
  tablePrefs.value = { ...tablePrefs.value, view: mode };
  void saveTablePrefs();
}

function onTableSort(next: { sort: string; order: "asc" | "desc" }) {
  sort.value = next.sort;
  sortOrder.value = next.order;
  offset.value = 0;
  void runSearch();
}

// Resizing fires continuously during a drag, so the width is applied
// live but only persisted once the pointer settles -- otherwise a
// single drag would POST a preferences blob per animation frame.
let widthSaveTimer: ReturnType<typeof setTimeout> | undefined;

function onColumnResize(next: { key: string; width: number }) {
  tablePrefs.value = { ...tablePrefs.value, widths: { ...tablePrefs.value.widths, [next.key]: clampWidth(next.width) } };
  clearTimeout(widthSaveTimer);
  widthSaveTimer = setTimeout(() => void saveTablePrefs(), 400);
}

const columnPickerOpen = ref(false);

function toggleColumn(key: string) {
  const current = tablePrefs.value.columns;
  const next = current.includes(key) ? current.filter((k) => k !== key) : [...current, key];
  tablePrefs.value = { ...tablePrefs.value, columns: next };
  void saveTablePrefs();
}

function moveColumn(key: string, delta: number) {
  const current = [...tablePrefs.value.columns];
  const from = current.indexOf(key);
  const to = from + delta;
  if (from === -1 || to < 0 || to >= current.length) return;
  [current[from], current[to]] = [current[to], current[from]];
  tablePrefs.value = { ...tablePrefs.value, columns: current };
  void saveTablePrefs();
}

function toolbarActionOrder(id: ToolbarActionId): number | undefined {
  const i = toolbarPrefs.value.order.indexOf(id);
  return i === -1 ? undefined : i;
}

// Virtual library / saved search management -- real, new routes (see
// crates/calibre_srv/src/lists.rs's own doc for why no upstream route
// exists to port here).
const manageOpen = ref(false);
const manageError = ref<string | null>(null);
const newVlName = ref("");
const newVlQuery = ref("");
const newSearchName = ref("");
const newSearchQuery = ref("");

function openManage() {
  newVlQuery.value = activeQuery.value;
  newSearchQuery.value = activeQuery.value;
  manageError.value = null;
  manageOpen.value = true;
}

async function createVirtualLibrary() {
  if (!newVlName.value.trim() || !newVlQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setVirtualLibrary(newVlName.value.trim(), newVlQuery.value.trim());
    newVlName.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeVirtualLibrary(name: string) {
  if (!confirm(`Delete the virtual library "${name}"?`)) return;
  manageError.value = null;
  try {
    await deleteVirtualLibrary(name);
    if (vl.value === name) vl.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function createSavedSearch() {
  if (!newSearchName.value.trim() || !newSearchQuery.value.trim()) return;
  manageError.value = null;
  try {
    await setSavedSearch(newSearchName.value.trim(), newSearchQuery.value.trim());
    newSearchName.value = "";
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeSavedSearch(name: string) {
  if (!confirm(`Delete the saved search "${name}"?`)) return;
  manageError.value = null;
  try {
    await deleteSavedSearch(name);
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

async function renameSavedSearchPrompt(name: string) {
  const newName = prompt("Rename saved search to:", name);
  if (!newName || newName === name) return;
  manageError.value = null;
  try {
    await renameSavedSearch(name, newName);
    await loadMetadata();
  } catch (e) {
    manageError.value = e instanceof Error ? e.message : String(e);
  }
}

// Custom column management -- real, new routes (see
// crates/calibre_srv/src/custom_columns.rs's own doc for why no
// upstream route exists to port here). `is_multiple` is left out of
// the create form entirely: it's not a real working combination for
// any datatype this backend supports yet (text/comments/series reject
// it outright, and bool/int/float/rating silently ignore it -- see
// that module's own doc) -- a real, disclosed narrowing rather than a
// checkbox that would look like it works but doesn't.
const columnsOpen = ref(false);
const columnsError = ref<string | null>(null);
const customColumns = ref<Record<string, CustomColumnInfo>>({});
const newColumnLabel = ref("");
const newColumnName = ref("");
const newColumnDatatype = ref("text");

async function loadCustomColumns() {
  try {
    customColumns.value = await fetchCustomColumns();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}

function openColumns() {
  columnsError.value = null;
  columnsOpen.value = true;
  void loadCustomColumns();
}

// Check Library (issue #748) -- real integrity scan, see
// crates/calibre_srv/src/check_library.rs's own doc.
const checkLibraryOpen = ref(false);
const checkLibraryLoading = ref(false);
const checkLibraryError = ref<string | null>(null);
const checkLibraryResult = ref<CheckLibraryResult | null>(null);

const checkLibraryNonEmpty = computed(() => {
  if (!checkLibraryResult.value) return [];
  return Object.entries(checkLibraryResult.value).filter(([, findings]) => findings.length > 0);
});

async function openCheckLibrary() {
  checkLibraryOpen.value = true;
  checkLibraryError.value = null;
  checkLibraryLoading.value = true;
  checkLibraryResult.value = null;
  try {
    checkLibraryResult.value = await checkLibrary();
  } catch (e) {
    checkLibraryError.value = e instanceof Error ? e.message : String(e);
  } finally {
    checkLibraryLoading.value = false;
  }
}

// Item notes (issue #732) -- see NoteEditor.vue's own doc.
const noteTarget = ref<{ field: string; itemName: string } | null>(null);

function openNote(field: string, itemName: string) {
  noteTarget.value = { field, itemName };
}

async function createCustomColumn() {
  if (!newColumnLabel.value.trim() || !newColumnName.value.trim()) return;
  columnsError.value = null;
  try {
    await addCustomColumn(newColumnLabel.value.trim(), newColumnName.value.trim(), newColumnDatatype.value);
    newColumnLabel.value = "";
    newColumnName.value = "";
    await loadCustomColumns();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}

async function removeCustomColumnClick(label: string) {
  if (!confirm(`Delete the custom column "${label}"? Its stored values will be lost.`)) return;
  columnsError.value = null;
  try {
    await removeCustomColumn(label);
    await loadCustomColumns();
  } catch (e) {
    columnsError.value = e instanceof Error ? e.message : String(e);
  }
}

function applySavedSearch(query: string) {
  queryText.value = query;
  activeQuery.value = query;
  offset.value = 0;
  manageOpen.value = false;
}

function exportCatalog() {
  // A plain navigation, not fetch+blob -- the real response's own
  // Content-Disposition: attachment header (catalog.rs) makes the
  // browser download it natively.
  window.open(catalogDownloadUrl(activeQuery.value), "_blank");
}

function exportLibraryArchive() {
  // Same plain-navigation pattern as exportCatalog -- library_export.rs's
  // own Content-Disposition header drives the real browser download.
  window.open(libraryExportUrl(), "_blank");
}

// Fetch news/recipes -- a real, generic RSS/Atom feed reader (see
// crates/calibre_srv/src/news.rs's own doc for why this isn't a
// catalog of upstream's ~1077 hand-written per-site recipes).
const newsOpen = ref(false);
const newsTitle = ref("");
const newsFeedUrls = ref("");
const newsOldestArticleDays = ref("");
const newsMaxArticlesPerFeed = ref("");
const newsFetching = ref(false);
const newsError = ref<string | null>(null);
const newsDone = ref(false);

// Scheduled feeds (#764) -- a saved feed configuration that runs
// automatically on a real recurring interval via a server-side
// background task, instead of only this panel's own one-shot fetch.
const newsSchedules = ref<NewsSchedule[]>([]);
const scheduleIntervalMinutes = ref("1440");
const schedulingBusy = ref(false);
const scheduleError = ref<string | null>(null);

async function loadNewsSchedules() {
  try {
    const { schedules } = await listNewsSchedules();
    newsSchedules.value = schedules;
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  }
}

function openNews() {
  newsOpen.value = true;
  newsError.value = null;
  newsDone.value = false;
  void loadNewsSchedules();
}

async function saveNewsSchedule() {
  // Real, disclosed narrowing: scheduled feeds (#764) only store plain
  // URLs, not #765's {title, url} sections -- strip any "Title|" prefix
  // rather than saving a broken literal URL.
  const feeds = parseFeedLines(newsFeedUrls.value).map((f) => (typeof f === "string" ? f : f.url));
  const minutes = Number(scheduleIntervalMinutes.value);
  if (feeds.length === 0 || !Number.isFinite(minutes) || minutes < 1) return;
  schedulingBusy.value = true;
  scheduleError.value = null;
  try {
    await addNewsSchedule(newsTitle.value.trim(), feeds, Math.round(minutes * 60));
    await loadNewsSchedules();
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  } finally {
    schedulingBusy.value = false;
  }
}

async function deleteNewsSchedule(id: number) {
  await removeNewsSchedule(id);
  await loadNewsSchedules();
}

async function runNewsScheduleNowClick(id: number) {
  schedulingBusy.value = true;
  scheduleError.value = null;
  try {
    const result = await runNewsScheduleNow(id);
    if (!result.ok) throw new Error(result.error || "run failed");
    cacheBust.value++;
    await runSearch();
    await loadNewsSchedules();
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  } finally {
    schedulingBusy.value = false;
  }
}

const opmlInput = ref<HTMLInputElement | null>(null);
const opmlImporting = ref(false);
const opmlSummary = ref<string | null>(null);

async function importOpmlFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  opmlImporting.value = true;
  opmlSummary.value = null;
  newsError.value = null;
  try {
    const text = await file.text();
    const feeds = await importOpml(text);
    if (feeds.length === 0) {
      opmlSummary.value = "No feeds found in that OPML file.";
      return;
    }
    const existing = newsFeedUrls.value.split("\n").map((u) => u.trim()).filter(Boolean);
    const newUrls = feeds.map((f) => f.feed_url).filter((u) => !existing.includes(u));
    newsFeedUrls.value = [...existing, ...newUrls].join("\n");
    opmlSummary.value = `Added ${newUrls.length} feed(s) from OPML${newUrls.length < feeds.length ? ` (${feeds.length - newUrls.length} already listed)` : ""}.`;
  } catch (e) {
    newsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    opmlImporting.value = false;
    if (opmlInput.value) opmlInput.value.value = "";
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// A line may be a bare URL, or "Section Title|https://..." naming its
// own real recipe section (#765) -- reuses the existing one-line-per-
// feed textarea rather than a new per-row form for this first slice.
function parseFeedLines(raw: string): NewsFeedInput[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const pipe = line.indexOf("|");
      if (pipe === -1) return line;
      const title = line.slice(0, pipe).trim();
      const url = line.slice(pipe + 1).trim();
      return title && url ? { title, url } : line;
    });
}

async function fetchNews() {
  const feeds = parseFeedLines(newsFeedUrls.value);
  if (feeds.length === 0) return;
  newsFetching.value = true;
  newsError.value = null;
  newsDone.value = false;
  try {
    const options: CustomRecipeOptions = {};
    if (newsOldestArticleDays.value.trim()) options.oldestArticleDays = Number(newsOldestArticleDays.value);
    if (newsMaxArticlesPerFeed.value.trim()) options.maxArticlesPerFeed = Number(newsMaxArticlesPerFeed.value);
    const jobId = await startNewsFetch(newsTitle.value.trim(), feeds, options);
    for (;;) {
      const status = await getNewsFetchStatus(jobId);
      if (!status.running) {
        if (!status.ok) throw new Error(status.error || "fetch failed");
        break;
      }
      await sleep(1000);
    }
    newsDone.value = true;
    cacheBust.value++;
    await runSearch();
  } catch (e) {
    newsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    newsFetching.value = false;
  }
}

async function runSearch() {
  loading.value = true;
  error.value = null;
  try {
    const result = await search({
      query: activeQuery.value,
      num: pageSize.value,
      offset: offset.value,
      sort: sort.value,
      sortOrder: sortOrder.value,
      vl: vl.value,
    });
    totalNum.value = result.total_num;
    books.value = await fetchBooks(result.book_ids);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
    books.value = [];
    totalNum.value = 0;
  } finally {
    loading.value = false;
  }
}

watch([activeQuery, sort, sortOrder, vl, offset], () => {
  if (!ftsMode.value) void runSearch();
}, { immediate: true });

// Full-text search -- a separate result shape/backend (crates/calibre_srv/src/fts.rs)
// from the metadata search above: no pagination/sort/vl, real
// snippet/highlight text per match, and a real "not enabled yet" state
// to surface instead of an empty result list.
interface FtsDisplayResult {
  bookId: number;
  title: string;
  authors: string;
  formats: string[];
  snippets: FtsSnippet[];
}

const ftsMode = ref(false);
const ftsLoading = ref(false);
const ftsError = ref<string | null>(null);
const ftsNotEnabled = ref(false);
const ftsEnabling = ref(false);
const ftsIndexing = ref<{ left: number; total: number } | null>(null);
const ftsResults = ref<FtsDisplayResult[]>([]);

async function runFtsSearch() {
  const query = activeQuery.value;
  ftsError.value = null;
  ftsNotEnabled.value = false;
  ftsIndexing.value = null;
  if (!query) {
    ftsResults.value = [];
    return;
  }
  ftsLoading.value = true;
  try {
    const outcome = await ftsSearch(query);
    if (!outcome.enabled) {
      ftsNotEnabled.value = true;
      ftsResults.value = [];
      return;
    }
    const { metadata, indexing_status, results } = outcome.result;
    ftsIndexing.value = indexing_status;

    const formatsByBook = new Map<number, string[]>();
    for (const r of results) {
      const list = formatsByBook.get(r.book_id) ?? [];
      if (!list.includes(r.format)) list.push(r.format);
      formatsByBook.set(r.book_id, list);
    }
    const bookIds = [...formatsByBook.keys()];
    const snippetsByBook = await ftsSnippets(bookIds, query);

    ftsResults.value = bookIds.map((bookId) => ({
      bookId,
      title: metadata[String(bookId)]?.title ?? `Book ${bookId}`,
      authors: metadata[String(bookId)]?.authors ?? "",
      formats: formatsByBook.get(bookId) ?? [],
      snippets: snippetsByBook[String(bookId)] ?? [],
    }));
  } catch (e) {
    ftsError.value = e instanceof Error ? e.message : String(e);
    ftsResults.value = [];
  } finally {
    ftsLoading.value = false;
  }
}

watch([activeQuery, ftsMode], () => {
  if (ftsMode.value) void runFtsSearch();
});

async function enableFts() {
  ftsEnabling.value = true;
  ftsError.value = null;
  try {
    await setFtsEnabled(true);
    await runFtsSearch();
  } catch (e) {
    ftsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    ftsEnabling.value = false;
  }
}

function snippetSegments(text: string) {
  return parseSnippetSegments(text);
}

function submitSearch() {
  activeQuery.value = queryText.value;
  offset.value = 0;
}

function onCategorySelect(categoryQuery: string) {
  queryText.value = categoryQuery;
  activeQuery.value = categoryQuery;
  offset.value = 0;
}

function nextPage() {
  if (offset.value + pageSize.value < totalNum.value) offset.value += pageSize.value;
}
function prevPage() {
  if (offset.value > 0) offset.value = Math.max(0, offset.value - pageSize.value);
}

function onDetailsUpdated() {
  cacheBust.value++;
  void (ftsMode.value ? runFtsSearch() : runSearch());
}

function onCategoryRenamed() {
  void (ftsMode.value ? runFtsSearch() : runSearch());
}

function onDetailsDeleted() {
  selectedBookId.value = null;
  void (ftsMode.value ? runFtsSearch() : runSearch());
}

// Bulk (multi-book) metadata editing -- client-side fan-out over the
// existing per-book /cdb/set-fields, matching #716's own scope: no
// bulk server endpoint exists (or is needed yet), just N real
// requests with real per-book error aggregation so one bad book
// doesn't silently swallow the rest.
const selectMode = ref(false);
const selectedIds = ref<Set<number>>(new Set());
const bulkOpen = ref(false);
const bulkBusy = ref(false);
const bulkErrors = ref<string[]>([]);
const bulkAddTags = ref("");
const bulkRemoveTags = ref("");
const bulkSeries = ref("");
const bulkApplyRating = ref(false);
const bulkRating = ref(0);

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) {
    selectedIds.value = new Set();
    bulkOpen.value = false;
  }
}

function toggleSelected(bookId: number) {
  const next = new Set(selectedIds.value);
  if (next.has(bookId)) next.delete(bookId);
  else next.add(bookId);
  selectedIds.value = next;
}

function onCardClick(bookId: number) {
  if (selectMode.value) toggleSelected(bookId);
  else selectedBookId.value = bookId;
}

async function runBulkEdit() {
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;

  const addTags = bulkAddTags.value.split(",").map((t) => t.trim()).filter(Boolean);
  const removeTags = bulkRemoveTags.value.split(",").map((t) => t.trim()).filter(Boolean);
  const setSeries = bulkSeries.value.trim();

  bulkBusy.value = true;
  bulkErrors.value = [];
  for (const id of ids) {
    try {
      const changes: BookFieldChanges = {};
      if (addTags.length || removeTags.length) {
        const current = books.value.find((b) => b.id === id);
        const tags = new Set(current?.tags ?? []);
        for (const t of addTags) tags.add(t);
        for (const t of removeTags) tags.delete(t);
        changes.tags = [...tags];
      }
      if (setSeries) changes.series = setSeries;
      if (bulkApplyRating.value) changes.rating = bulkRating.value;
      if (Object.keys(changes).length === 0) continue;
      await setFields(id, changes);
    } catch (e) {
      bulkErrors.value.push(`Book ${id}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  bulkBusy.value = false;

  if (bulkErrors.value.length === 0) {
    bulkOpen.value = false;
    selectMode.value = false;
    selectedIds.value = new Set();
    bulkAddTags.value = "";
    bulkRemoveTags.value = "";
    bulkSeries.value = "";
    bulkApplyRating.value = false;
  }
  cacheBust.value++;
  await runSearch();
}

// Save to disk (#751) -- real POST /save-to-disk against whatever
// books are currently selected (falls back to the single currently
// open book if selection mode isn't active), evaluating a real
// calibre-template-language `{field}`-shorthand path template per
// book server-side.
const saveToDiskOpen = ref(false);
const saveToDiskTemplate = ref("{author_sort}/{title}/{title} - {authors}");
const saveToDiskDest = ref("");
const saveToDiskBusy = ref(false);
const saveToDiskResults = ref<SaveToDiskResult[]>([]);
const saveToDiskError = ref<string | null>(null);

function openSaveToDisk() {
  saveToDiskOpen.value = true;
  saveToDiskResults.value = [];
  saveToDiskError.value = null;
}

async function runSaveToDisk() {
  const ids = selectMode.value && selectedIds.value.size > 0 ? [...selectedIds.value] : selectedBookId.value !== null ? [selectedBookId.value] : [];
  if (ids.length === 0 || !saveToDiskDest.value.trim()) return;
  saveToDiskBusy.value = true;
  saveToDiskError.value = null;
  saveToDiskResults.value = [];
  try {
    const { results } = await saveToDisk(ids, saveToDiskTemplate.value, saveToDiskDest.value.trim());
    saveToDiskResults.value = results;
  } catch (e) {
    saveToDiskError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saveToDiskBusy.value = false;
  }
}

// Find duplicates (#762) -- real POST /duplicates/scan against the
// whole library, reusing calibre_db::copy_to_library's existing
// add-time collision-detection heuristic as a whole-library scan.
const duplicatesOpen = ref(false);
const duplicatesLoading = ref(false);
const duplicatesError = ref<string | null>(null);
const duplicateGroups = ref<DuplicateBook[][]>([]);

async function openDuplicates() {
  duplicatesOpen.value = true;
  duplicatesError.value = null;
  duplicatesLoading.value = true;
  try {
    const { groups } = await scanForDuplicates();
    duplicateGroups.value = groups;
  } catch (e) {
    duplicatesError.value = e instanceof Error ? e.message : String(e);
  } finally {
    duplicatesLoading.value = false;
  }
}

async function deleteDuplicateBook(bookId: number) {
  if (!confirm(`Delete book ${bookId}? This cannot be undone.`)) return;
  await deleteBooks([bookId]);
  duplicateGroups.value = duplicateGroups.value.map((g) => g.filter((b) => b.book_id !== bookId)).filter((g) => g.length > 1);
  cacheBust.value++;
  await runSearch();
}

const addSummary = ref<string | null>(null);

async function addOneBookFile(file: File, addDuplicates: boolean): Promise<"added" | "skipped"> {
  const result = await addBook(file, addDuplicates);
  if (result.duplicates && result.duplicates.length > 0 && result.book_id === undefined) {
    // Issue #721's real behavior change: "add"/"skip" act immediately
    // with no prompt; "ask" (the default) keeps the pre-#721 confirm()
    // dialog.
    if (duplicateDefault.value === "add") return addOneBookFile(file, true);
    if (duplicateDefault.value === "skip") return "skipped";
    const names = result.duplicates.map((d) => `${d.title} (${d.authors.join(" & ")})`).join(", ");
    if (confirm(`"${file.name}": a book with the same title/author already exists: ${names}. Add anyway?`)) {
      return addOneBookFile(file, true);
    }
    return "skipped";
  }
  return "added";
}

async function onAddFileSelected(e: Event) {
  const input = e.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  if (files.length === 0) return;
  adding.value = true;
  addError.value = null;
  addSummary.value = null;
  const errors: string[] = [];
  let added = 0;
  let skipped = 0;
  for (const file of files) {
    try {
      const outcome = await addOneBookFile(file, false);
      if (outcome === "added") added++;
      else skipped++;
    } catch (err) {
      errors.push(`${file.name}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  adding.value = false;
  if (addInput.value) addInput.value.value = "";
  if (errors.length > 0) addError.value = errors.join("; ");
  if (files.length > 1) {
    addSummary.value = `Added ${added} of ${files.length} book(s)${skipped ? `, ${skipped} skipped` : ""}${errors.length ? `, ${errors.length} failed` : ""}.`;
  }
  cacheBust.value++;
  await runSearch();
}

interface AddFolderResult {
  added: number;
  duplicates: string[];
  errors: string[];
}

const addingFolder = ref(false);

async function addFolder() {
  addingFolder.value = true;
  addError.value = null;
  addSummary.value = null;
  try {
    const result = await tauriInvoke<AddFolderResult | null>("choose_folder_and_add_books");
    if (!result) return; // dialog cancelled
    addSummary.value = `Added ${result.added} book(s) from the folder${result.duplicates.length ? `, skipped ${result.duplicates.length} duplicate(s)` : ""}${result.errors.length ? `, ${result.errors.length} failed` : ""}.`;
    if (result.errors.length > 0) addError.value = result.errors.join("; ");
    cacheBust.value++;
    await runSearch();
  } catch (e) {
    addError.value = e instanceof Error ? e.message : String(e);
  } finally {
    addingFolder.value = false;
  }
}

// Drag-and-drop add (#818). The drop itself is handled natively --
// Tauri's own drag-drop handling suppresses the webview's HTML5 drop
// events, so there is nothing for the page to listen for until the
// Rust side has already done the work and says so here.
//
// The page has to be told explicitly because it does not subscribe to
// calibre_srv's websocket, so the `BooksAdded` change event the add
// endpoint publishes never reaches it.
function onBooksDropped(event: Event) {
  const detail = (event as CustomEvent<AddFolderResult & { error?: string }>).detail;
  if (!detail) return;
  if (detail.error) {
    addError.value = detail.error;
    return;
  }
  addSummary.value = `Added ${detail.added} book(s)${detail.duplicates.length ? `, skipped ${detail.duplicates.length} duplicate(s)` : ""}${detail.errors.length ? `, ${detail.errors.length} failed` : ""}.`;
  addError.value = detail.errors.length > 0 ? detail.errors.join("; ") : null;
  cacheBust.value++;
  void runSearch();
}

window.addEventListener("oxide:books-added", onBooksDropped);

// Switch-library quick-switch (issue #725, desktop-only). This app
// stays single-library-per-instance -- "switching" re-spawns
// calibre_srv against a different path and re-navigates the window
// (app/src-tauri/src/lib.rs's open_library/open_recent_library), it
// doesn't serve multiple libraries at once. The list is capped/
// deduplicated server-side (app/src-tauri/src/settings.rs).
const switchOpen = ref(false);
const recentLibraries = ref<string[]>([]);
const switching = ref(false);
const switchError = ref<string | null>(null);

async function openSwitchLibrary() {
  switchError.value = null;
  switchOpen.value = true;
  try {
    recentLibraries.value = await tauriInvoke<string[]>("list_recent_libraries");
  } catch (e) {
    switchError.value = e instanceof Error ? e.message : String(e);
  }
}

async function switchToRecent(path: string) {
  switching.value = true;
  switchError.value = null;
  try {
    await tauriInvoke<void>("open_recent_library", { path });
    switchOpen.value = false;
  } catch (e) {
    switchError.value = e instanceof Error ? e.message : String(e);
  } finally {
    switching.value = false;
  }
}

async function switchToOther() {
  switching.value = true;
  switchError.value = null;
  try {
    const picked = await tauriInvoke<boolean>("choose_library");
    if (picked) switchOpen.value = false;
  } catch (e) {
    switchError.value = e instanceof Error ? e.message : String(e);
  } finally {
    switching.value = false;
  }
}

// ---------------------------------------------------------------
// Action registry wiring (#817)
// ---------------------------------------------------------------
//
// This view's half of the registry: what each action *does*. The
// registry itself (library/actions.ts) is data only, so that the
// settings panel and the desktop native menu can read the catalogue
// without importing this component's state.
//
// Anything not listed here simply does not render, which is what
// keeps the catalogue honest: declaring an action in the registry
// does not conjure a button for it.

const actionHandlers: Partial<Record<LibraryActionId, () => void>> = {
  "manage-lists": () => openManage(),
  "custom-columns": () => openColumns(),
  "check-library": () => openCheckLibrary(),
  "find-duplicates": () => openDuplicates(),
  "export-catalog": () => exportCatalog(),
  "export-library-archive": () => exportLibraryArchive(),
  "fetch-news": () => openNews(),
  "add-books": () => addInput.value?.click(),
  "add-folder": () => void addFolder(),
  "switch-library": () => openSwitchLibrary(),
  "select-mode": () => toggleSelectMode(),
  "bulk-edit": () => {
    bulkOpen.value = true;
  },
  "save-to-disk": () => openSaveToDisk(),
};

// Full-text search replaces the whole result area with a different
// list, so the library-management panels and the selection actions --
// all of which act on the metadata grid -- have never been shown
// alongside it. Adding/switching libraries stayed available, and
// still does.
//
// This is a LibraryView-specific mode rather than a property of the
// actions themselves, so it is filtered here instead of becoming a
// field in the shared registry.
const HIDDEN_IN_FTS_MODE: ReadonlySet<LibraryActionId> = new Set<LibraryActionId>([
  "manage-lists",
  "custom-columns",
  "check-library",
  "find-duplicates",
  "export-catalog",
  "export-library-archive",
  "fetch-news",
  "select-mode",
  "bulk-edit",
  "save-to-disk",
]);

const actionContext = computed<ActionContext>(() => ({
  selectionCount: selectMode.value ? selectedIds.value.size : selectedBookId.value !== null ? 1 : 0,
  isDesktop: isTauri(),
}));

/**
 * Per-action presentation that depends on live state. The registry
 * supplies the stable label; anything that changes while the user
 * watches (an in-flight "Adding…", a selection count, a tooltip that
 * depends on whether a search is active) is layered on here.
 */
function actionLabel(action: LibraryAction): string {
  switch (action.id) {
    case "add-books":
      return adding.value ? "Adding…" : action.label;
    case "add-folder":
      return addingFolder.value ? "Adding…" : action.label;
    case "select-mode":
      return selectMode.value ? "Cancel selection" : action.label;
    case "bulk-edit":
    case "save-to-disk":
      return `${action.label} (${selectedIds.value.size})`;
    default:
      return action.label;
  }
}

function actionBusy(action: LibraryAction): boolean {
  if (action.id === "add-books") return adding.value;
  if (action.id === "add-folder") return addingFolder.value;
  return false;
}

function actionTitle(action: LibraryAction): string | undefined {
  switch (action.id) {
    case "export-catalog":
      return activeQuery.value ? "Export the current search results as a CSV catalog" : "Export the whole library as a CSV catalog";
    case "export-library-archive":
      return "Download the whole library (every book and its metadata) as a real .zip archive for backup or transfer";
    default:
      return undefined;
  }
}

/** `select-mode` is a toggle, so it reflects its on state. */
function actionActive(action: LibraryAction): boolean {
  return action.id === "select-mode" && selectMode.value;
}

const toolbarActions = computed<LibraryAction[]>(() =>
  visibleToolbarActions({
    handled: Object.keys(actionHandlers) as LibraryActionId[],
    hidden: toolbarPrefs.value.hidden,
    suppressed: ftsMode.value ? HIDDEN_IN_FTS_MODE : undefined,
    ctx: actionContext.value,
  }),
);

function runAction(id: LibraryActionId) {
  const handler = actionHandlers[id];
  if (!handler) return;
  const action = LIBRARY_ACTIONS.find((a) => a.id === id);
  if (action && !actionEnabled(action, actionContext.value)) return;
  handler();
}

// Keep the desktop app's native menu in step with what this view can
// currently do. Re-sent whenever enablement could have changed; a
// no-op in a plain browser tab.
watch(
  [actionContext, ftsMode, toolbarPrefs],
  () => {
    void syncDesktopMenu(Object.keys(actionHandlers) as LibraryActionId[], actionContext.value);
  },
  { immediate: true, deep: true },
);

onMenuAction(runAction);

// ---------------------------------------------------------------
// Right-click menu (#1.2)
// ---------------------------------------------------------------
//
// Book-scoped actions are implemented by BookDetailsPanel, so the
// menu selects the book and hands the panel an action to perform
// rather than duplicating a dozen controls here. Selection-scoped
// actions this view owns outright.

const contextMenu = ref<{ x: number; y: number } | null>(null);
const pendingBookAction = ref<{ id: string; nonce: number } | null>(null);
let actionNonce = 0;

/** Book actions BookDetailsPanel knows how to perform. */
const PANEL_ACTIONS: LibraryActionId[] = ["read", "edit-metadata", "fetch-metadata", "convert", "tweak-book", "quick-view", "test-template", "send-email", "replace-cover", "open-externally"];

const contextEntries = computed(() => contextMenuEntries([...PANEL_ACTIONS, "bulk-edit", "save-to-disk", "delete-book"], actionContext.value));

function openContextMenu(payload: { bookId: number; x: number; y: number }) {
  // Right-clicking a row that is not part of the current selection
  // acts on that row, matching every file manager: the click moves
  // the selection first, then the menu describes it.
  if (!selectMode.value) selectedBookId.value = payload.bookId;
  else if (!selectedIds.value.has(payload.bookId)) toggleSelected(payload.bookId);
  contextMenu.value = { x: payload.x, y: payload.y };
}

async function deleteSelectedBooks() {
  const ids = selectMode.value && selectedIds.value.size > 0 ? [...selectedIds.value] : selectedBookId.value !== null ? [selectedBookId.value] : [];
  if (ids.length === 0) return;
  if (!window.confirm(`Delete ${ids.length} book(s)? This cannot be undone.`)) return;
  try {
    await deleteBooks(ids);
    if (selectedBookId.value !== null && ids.includes(selectedBookId.value)) selectedBookId.value = null;
    selectedIds.value = new Set();
    await runSearch();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

function onContextChoose(id: LibraryActionId) {
  if (id === "delete-book") {
    void deleteSelectedBooks();
    return;
  }
  if (id === "bulk-edit" || id === "save-to-disk") {
    runAction(id);
    return;
  }
  // Everything else belongs to the details panel.
  actionNonce += 1;
  pendingBookAction.value = { id, nonce: actionNonce };
}

// ---------------------------------------------------------------
// Keyboard shortcuts (#1.3)
// ---------------------------------------------------------------
//
// The third surface over the registry. Until now the keymap held two
// bindings, both reader-only, and this view had no keydown handler at
// all -- so the shortcuts settings panel configured almost nothing.
//
// The decision of whether a key press should fire lives in
// library/shortcuts.ts, because the part worth getting right is when
// *not* to act: a bare "a" must not steal the letter you are typing
// into the search box.

const keymap = ref<KeymapPrefs>({ ...DEFAULT_KEYMAP });

async function loadKeymap() {
  try {
    const prefs = await fetchProfile<KeymapPrefs>(KEYMAP_PROFILE);
    if (prefs) keymap.value = { ...DEFAULT_KEYMAP, ...prefs };
  } catch (e) {
    console.error("failed to load keyboard shortcuts", e);
  }
}
void loadKeymap();

const searchInput = ref<HTMLInputElement | null>(null);

function onLibraryKeydown(event: KeyboardEvent) {
  // A modal owns the keyboard while it is open -- firing library
  // shortcuts underneath one would act on a view the user cannot
  // currently see.
  if (contextMenu.value || manageOpen.value || columnsOpen.value || columnPickerOpen.value || checkLibraryOpen.value || duplicatesOpen.value || saveToDiskOpen.value || newsOpen.value || switchOpen.value) return;

  const id = shortcutFor(event, libraryShortcuts(keymap.value));
  if (!id) return;

  event.preventDefault();
  if (id === "focus-search") {
    searchInput.value?.focus();
    searchInput.value?.select();
    return;
  }
  if (id === "toggle-view") {
    setViewMode(viewMode.value === "table" ? "grid" : "table");
    return;
  }
  if (id === "delete-book") {
    void deleteSelectedBooks();
    return;
  }
  runAction(id);
}

onMounted(() => window.addEventListener("keydown", onLibraryKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onLibraryKeydown));
</script>

<template>
  <div class="library">
    <header class="toolbar">
      <form class="search" @submit.prevent="submitSearch">
        <input ref="searchInput" v-model="queryText" type="search" :placeholder="ftsMode ? 'Search book contents…' : 'Search…'" />
        <button type="submit">Search</button>
      </form>

      <button type="button" :class="{ active: ftsMode }" @click="ftsMode = !ftsMode">
        {{ ftsMode ? "Full-text search" : "Metadata search" }}
      </button>

      <template v-if="!ftsMode">
        <div class="sort-fields">
          <select v-model="primarySort">
            <option v-for="[key, label] in sortableFields" :key="key" :value="key">{{ label }}</option>
          </select>
          <span v-for="field in secondarySortFields" :key="field" class="sort-chip">
            {{ fieldLabel(field) }}
            <button type="button" class="sort-chip-remove" :aria-label="`Stop sorting by ${fieldLabel(field)}`" @click="removeSortField(field)">✕</button>
          </span>
          <select v-if="availableExtraSortFields.length" :value="''" @change="addSortField(($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''">
            <option value="" disabled>+ then sort by…</option>
            <option v-for="[key, label] in availableExtraSortFields" :key="key" :value="key">{{ label }}</option>
          </select>
        </div>
        <select v-model="sortOrder">
          <option value="asc">Ascending</option>
          <option value="desc">Descending</option>
        </select>
        <select v-model="vl">
          <option value="">All books</option>
          <option v-for="name in Object.keys(virtualLibraries)" :key="name" :value="name">{{ name }}</option>
        </select>
        <select v-if="Object.keys(savedSearches).length" @change="applySavedSearch(($event.target as HTMLSelectElement).value)">
          <option value="" disabled selected>Saved searches…</option>
          <option v-for="[name, q] in Object.entries(savedSearches)" :key="name" :value="q">{{ name }}</option>
        </select>
        <div class="view-toggle" role="group" aria-label="Library view">
          <button type="button" :class="{ active: viewMode === 'table' }" :aria-pressed="viewMode === 'table'" @click="setViewMode('table')">Table</button>
          <button type="button" :class="{ active: viewMode === 'grid' }" :aria-pressed="viewMode === 'grid'" @click="setViewMode('grid')">Grid</button>
        </div>
        <button v-if="viewMode === 'table'" type="button" @click="columnPickerOpen = true">Columns…</button>
      </template>

      <!--
        Every action button comes from the registry (#817). Before
        this there were thirteen near-identical hardcoded buttons,
        each repeating its own visibility and ordering lookup; the
        registry is what the context menu, keyboard shortcuts and the
        desktop native menu all read from, so rendering the toolbar
        from it too is what keeps those four surfaces in agreement.
      -->
      <button
        v-for="action in toolbarActions"
        :key="action.id"
        type="button"
        :style="{ order: toolbarActionOrder(action.id) }"
        :class="{ active: actionActive(action) }"
        :disabled="actionBusy(action)"
        :title="actionTitle(action)"
        @click="runAction(action.id)"
      >
        {{ actionLabel(action) }}
      </button>
      <input ref="addInput" type="file" multiple class="hidden-file-input" @change="onAddFileSelected" />
      <router-link to="/settings" class="settings-link">Settings…</router-link>
    </header>

    <div v-if="bulkOpen" class="bulk-panel">
      <div class="bulk-row">
        <label>Add tags <input v-model="bulkAddTags" placeholder="scifi, classic" :disabled="bulkBusy" /></label>
        <label>Remove tags <input v-model="bulkRemoveTags" placeholder="unwanted-tag" :disabled="bulkBusy" /></label>
        <label>Set series <input v-model="bulkSeries" :disabled="bulkBusy" /></label>
        <label class="bulk-rating">
          <input type="checkbox" v-model="bulkApplyRating" :disabled="bulkBusy" />
          Set rating
          <input type="number" v-model.number="bulkRating" min="0" max="5" step="1" :disabled="bulkBusy || !bulkApplyRating" />
        </label>
      </div>
      <div class="bulk-actions">
        <button type="button" :disabled="bulkBusy" @click="runBulkEdit">
          {{ bulkBusy ? "Applying…" : `Apply to ${selectedIds.size} book(s)` }}
        </button>
        <button type="button" :disabled="bulkBusy" @click="bulkOpen = false">Close</button>
      </div>
      <ul v-if="bulkErrors.length" class="bulk-errors">
        <li v-for="(err, i) in bulkErrors" :key="i" class="error">{{ err }}</li>
      </ul>
    </div>

    <p v-if="addError" class="error add-error">{{ addError }}</p>
    <p v-if="addSummary" class="status add-summary">{{ addSummary }}</p>

    <div class="body">
      <CategoryBrowser class="sidebar" @select="onCategorySelect" @view-note="openNote" @renamed="onCategoryRenamed" />

      <main v-if="ftsMode" class="grid-area">
        <p v-if="ftsError" class="error">{{ ftsError }}</p>
        <div v-else-if="ftsNotEnabled" class="fts-enable">
          <p>Full-text search is not enabled on this library yet.</p>
          <button type="button" :disabled="ftsEnabling" @click="enableFts">
            {{ ftsEnabling ? "Enabling…" : "Enable full-text search" }}
          </button>
        </div>
        <template v-else>
          <p v-if="ftsIndexing && ftsIndexing.left > 0" class="status fts-indexing">
            Indexing… {{ ftsIndexing.left }} of {{ ftsIndexing.total }} remaining. Results may be incomplete until indexing finishes.
          </p>
          <p v-if="ftsLoading" class="status">Searching…</p>
          <p v-else-if="activeQuery && ftsResults.length === 0" class="status">No matches found.</p>
          <p v-else-if="!activeQuery" class="status">Enter a search to look through book contents.</p>

          <ul class="fts-results">
            <li v-for="r in ftsResults" :key="r.bookId" class="fts-result" @click="selectedBookId = r.bookId">
              <div class="fts-result-header">
                <span class="fts-result-title">{{ r.title }}</span>
                <span class="fts-result-authors">{{ r.authors }}</span>
                <span class="fts-result-formats">{{ r.formats.join(", ") }}</span>
              </div>
              <p v-for="(snippet, i) in r.snippets" :key="i" class="fts-snippet">
                <template v-for="(seg, j) in snippetSegments(snippet.text)" :key="j">
                  <mark v-if="seg.highlighted">{{ seg.text }}</mark>
                  <template v-else>{{ seg.text }}</template>
                </template>
              </p>
            </li>
          </ul>
        </template>
      </main>

      <main v-else class="grid-area">
        <p v-if="error" class="error">{{ error }}</p>
        <p v-else-if="loading" class="status">Loading…</p>
        <p v-else-if="books.length === 0" class="status">No books found.</p>

        <BookTable
          v-if="viewMode === 'table'"
          :books="books"
          :columns="visibleColumns"
          :select-mode="selectMode"
          :selected-ids="selectedIds"
          :selected-book-id="selectedBookId"
          :sort="sort"
          :sort-order="sortOrder"
          @select="selectedBookId = $event"
          @toggle-selected="toggleSelected"
          @sort-by="onTableSort"
          @resize="onColumnResize"
          @context-menu="openContextMenu"
        />

        <div v-else class="grid">
          <button v-for="book in books" :key="book.id" class="card" :class="{ selected: selectMode && selectedIds.has(book.id) }" @click="onCardClick(book.id)" @contextmenu.prevent="openContextMenu({ bookId: book.id, x: $event.clientX, y: $event.clientY })">
            <input v-if="selectMode" type="checkbox" class="card-checkbox" :checked="selectedIds.has(book.id)" @click.stop="toggleSelected(book.id)" />
            <img :src="`${book.thumbnail}?v=${cacheBust}`" :alt="book.title" loading="lazy" />
            <div class="card-title">{{ book.title }}</div>
            <div class="card-authors">{{ (book.authors ?? []).join(" & ") }}</div>
          </button>
        </div>

        <footer class="pagination">
          <button :disabled="offset === 0" @click="prevPage">◀ Prev</button>
          <span>Page {{ currentPage }} / {{ pageCount }} ({{ totalNum }} books)</span>
          <button :disabled="offset + pageSize >= totalNum" @click="nextPage">Next ▶</button>
        </footer>
      </main>
    </div>

    <ContextMenu v-if="contextMenu" :x="contextMenu.x" :y="contextMenu.y" :entries="contextEntries" @choose="onContextChoose" @close="contextMenu = null" />

    <BookDetailsPanel v-if="selectedBookId !== null" :book-id="selectedBookId" :pending-action="pendingBookAction" @close="selectedBookId = null" @updated="onDetailsUpdated" @deleted="onDetailsDeleted" @open-book="selectedBookId = $event" />
    <NoteEditor v-if="noteTarget" :field="noteTarget.field" :item-name="noteTarget.itemName" @close="noteTarget = null" />

    <div v-if="manageOpen" class="manage-backdrop" @click.self="manageOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="manageOpen = false">✕</button>
        <p v-if="manageError" class="error">{{ manageError }}</p>

        <section>
          <h3>Virtual libraries</h3>
          <ul class="manage-list">
            <li v-for="[name, q] in Object.entries(virtualLibraries)" :key="name">
              <span class="manage-name">{{ name }}</span>
              <code class="manage-query">{{ q }}</code>
              <button type="button" class="manage-remove" @click="removeVirtualLibrary(name)">Delete</button>
            </li>
          </ul>
          <form class="manage-form" @submit.prevent="createVirtualLibrary">
            <input v-model="newVlName" placeholder="Name" required />
            <input v-model="newVlQuery" placeholder="Search query" required />
            <button type="submit">Add</button>
          </form>
        </section>

        <section>
          <h3>Saved searches</h3>
          <ul class="manage-list">
            <li v-for="[name, q] in Object.entries(savedSearches)" :key="name">
              <span class="manage-name">{{ name }}</span>
              <code class="manage-query">{{ q }}</code>
              <button type="button" @click="renameSavedSearchPrompt(name)">Rename</button>
              <button type="button" class="manage-remove" @click="removeSavedSearch(name)">Delete</button>
            </li>
          </ul>
          <form class="manage-form" @submit.prevent="createSavedSearch">
            <input v-model="newSearchName" placeholder="Name" required />
            <input v-model="newSearchQuery" placeholder="Search query" required />
            <button type="submit">Add</button>
          </form>
        </section>
      </div>
    </div>

    <div v-if="columnPickerOpen" class="manage-backdrop" @click.self="columnPickerOpen = false">
      <div class="manage-panel">
        <h3>Table columns</h3>
        <p class="hint">Choose which columns the table shows, and their order.</p>
        <ul class="column-list">
          <li v-for="column in availableColumns" :key="column.key" class="column-row">
            <label class="field checkbox">
              <input type="checkbox" :checked="tablePrefs.columns.includes(column.key)" @change="toggleColumn(column.key)" />
              {{ column.label }}
              <span v-if="!column.sortKey" class="hint inline">(not sortable)</span>
            </label>
            <template v-if="tablePrefs.columns.includes(column.key)">
              <button type="button" :disabled="tablePrefs.columns.indexOf(column.key) === 0" title="Move left" @click="moveColumn(column.key, -1)">←</button>
              <button type="button" :disabled="tablePrefs.columns.indexOf(column.key) === tablePrefs.columns.length - 1" title="Move right" @click="moveColumn(column.key, 1)">→</button>
            </template>
          </li>
        </ul>
        <button type="button" @click="columnPickerOpen = false">Close</button>
      </div>
    </div>

    <div v-if="columnsOpen" class="manage-backdrop" @click.self="columnsOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="columnsOpen = false">✕</button>
        <section>
          <h3>Custom columns</h3>
          <p v-if="columnsError" class="error">{{ columnsError }}</p>
          <ul class="manage-list">
            <li v-for="[label, col] in Object.entries(customColumns)" :key="label">
              <span class="manage-name">{{ col.name }}</span>
              <code class="manage-query">#{{ label }} ({{ col.datatype }})</code>
              <button type="button" class="manage-remove" @click="removeCustomColumnClick(label)">Delete</button>
            </li>
            <li v-if="Object.keys(customColumns).length === 0">No custom columns yet.</li>
          </ul>
          <form class="manage-form" @submit.prevent="createCustomColumn">
            <input v-model="newColumnLabel" placeholder="Label (e.g. shelf)" pattern="[a-zA-Z0-9_]+" title="Letters, numbers, and underscores only" required />
            <input v-model="newColumnName" placeholder="Display name (e.g. Shelf)" required />
            <select v-model="newColumnDatatype">
              <option value="text">Text</option>
              <option value="comments">Long text</option>
              <option value="int">Integer</option>
              <option value="float">Decimal number</option>
              <option value="bool">Yes/No</option>
              <option value="rating">Rating</option>
            </select>
            <button type="submit">Add</button>
          </form>
        </section>
      </div>
    </div>

    <div v-if="checkLibraryOpen" class="manage-backdrop" @click.self="checkLibraryOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="checkLibraryOpen = false">✕</button>
        <section>
          <h3>Check library</h3>
          <p v-if="checkLibraryLoading">Scanning…</p>
          <p v-else-if="checkLibraryError" class="error">{{ checkLibraryError }}</p>
          <template v-else-if="checkLibraryResult">
            <p v-if="checkLibraryNonEmpty.length === 0" class="news-hint">No problems found.</p>
            <div v-for="[key, findings] in checkLibraryNonEmpty" :key="key">
              <h4>{{ CHECK_LIBRARY_LABELS[key] ?? key }} ({{ findings.length }})</h4>
              <ul class="manage-list">
                <li v-for="(f, i) in findings" :key="i">
                  <span class="manage-name">{{ f.a }}</span>
                  <code class="manage-query">{{ f.b }}</code>
                </li>
              </ul>
            </div>
          </template>
        </section>
      </div>
    </div>

    <div v-if="duplicatesOpen" class="manage-backdrop" @click.self="duplicatesOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="duplicatesOpen = false">✕</button>
        <section>
          <h3>Find duplicates</h3>
          <p v-if="duplicatesLoading">Scanning…</p>
          <p v-else-if="duplicatesError" class="error">{{ duplicatesError }}</p>
          <template v-else>
            <p v-if="duplicateGroups.length === 0" class="news-hint">No likely duplicates found.</p>
            <div v-for="(group, i) in duplicateGroups" :key="i">
              <h4>Group {{ i + 1 }} ({{ group.length }} books)</h4>
              <ul class="manage-list">
                <li v-for="b in group" :key="b.book_id">
                  <button type="button" @click="selectedBookId = b.book_id">{{ b.title }} — {{ b.authors.join(", ") }}</button>
                  <button type="button" @click="deleteDuplicateBook(b.book_id)">Delete</button>
                </li>
              </ul>
            </div>
          </template>
        </section>
      </div>
    </div>

    <div v-if="saveToDiskOpen" class="manage-backdrop" @click.self="saveToDiskOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="saveToDiskOpen = false">✕</button>
        <h3>Save to disk</h3>
        <p class="news-hint">Exports the selected book(s) into a folder tree named from a real calibre template. Available fields include {title}, {authors}, {author_sort}, {series}, and any custom column (e.g. {#shelf}).</p>
        <form @submit.prevent="runSaveToDisk">
          <label class="news-field">
            Path template
            <input v-model="saveToDiskTemplate" placeholder="{author_sort}/{title}/{title} - {authors}" :disabled="saveToDiskBusy" />
          </label>
          <label class="news-field">
            Destination folder (absolute path)
            <input v-model="saveToDiskDest" placeholder="/home/me/Books" :disabled="saveToDiskBusy" />
          </label>
          <div class="bulk-actions">
            <button type="submit" class="read" :disabled="saveToDiskBusy || !saveToDiskDest.trim()">{{ saveToDiskBusy ? "Saving…" : "Save" }}</button>
            <button type="button" :disabled="saveToDiskBusy" @click="saveToDiskOpen = false">Close</button>
          </div>
        </form>
        <p v-if="saveToDiskError" class="error">{{ saveToDiskError }}</p>
        <ul v-if="saveToDiskResults.length" class="manage-list">
          <li v-for="r in saveToDiskResults" :key="r.book_id">
            <span v-if="r.ok" class="manage-name">Book {{ r.book_id }}: saved {{ r.paths?.length }} file(s)</span>
            <span v-else class="error">Book {{ r.book_id }}: {{ r.error }}</span>
          </li>
        </ul>
      </div>
    </div>

    <div v-if="newsOpen" class="manage-backdrop" @click.self="newsOpen = false">
      <div class="manage-panel news-panel">
        <button class="manage-close" @click="newsOpen = false">✕</button>
        <h3>Fetch news</h3>
        <p class="news-hint">Enter one or more RSS/Atom feed URLs, one per line. Prefix a line with "Section Title|" to name that feed's own section in a multi-section recipe. This downloads the latest articles and adds the result as a new book.</p>
        <form @submit.prevent="fetchNews">
          <label class="news-field">
            Title
            <input v-model="newsTitle" placeholder="My Weekly" :disabled="newsFetching" />
          </label>
          <label class="news-field">
            Feed URLs
            <textarea v-model="newsFeedUrls" rows="4" placeholder="https://example.com/feed.xml&#10;Tech News|https://example.com/tech.xml" :disabled="newsFetching"></textarea>
          </label>
          <div class="bulk-actions">
            <button type="button" :disabled="newsFetching || opmlImporting" @click="opmlInput?.click()">{{ opmlImporting ? "Importing…" : "Import OPML…" }}</button>
            <input ref="opmlInput" type="file" accept=".opml,.xml,text/x-opml,text/xml" class="hidden-file-input" @change="importOpmlFile" />
          </div>
          <label class="news-field">
            Oldest article (days)
            <input v-model="newsOldestArticleDays" type="number" min="0" step="1" placeholder="7" :disabled="newsFetching" />
          </label>
          <label class="news-field">
            Max articles per feed
            <input v-model="newsMaxArticlesPerFeed" type="number" min="1" step="1" placeholder="100" :disabled="newsFetching" />
          </label>
          <label class="news-field">
            Repeat every (minutes)
            <input v-model="scheduleIntervalMinutes" type="number" min="1" step="1" :disabled="schedulingBusy" />
          </label>
          <div class="bulk-actions">
            <button type="submit" class="read" :disabled="newsFetching || !newsFeedUrls.trim()">{{ newsFetching ? "Fetching…" : "Fetch once" }}</button>
            <button type="button" :disabled="schedulingBusy || !newsFeedUrls.trim()" @click="saveNewsSchedule">{{ schedulingBusy ? "Saving…" : "Save schedule…" }}</button>
            <button type="button" :disabled="newsFetching" @click="newsOpen = false">Close</button>
          </div>
        </form>
        <p v-if="opmlSummary" class="news-hint">{{ opmlSummary }}</p>
        <p v-if="newsDone" class="news-done">Added the fetched news as a new book.</p>
        <p v-if="newsError" class="error">{{ newsError }}</p>
        <p v-if="scheduleError" class="error">{{ scheduleError }}</p>

        <template v-if="newsSchedules.length">
          <h4>Scheduled feeds</h4>
          <ul class="manage-list">
            <li v-for="s in newsSchedules" :key="s.id">
              <span class="manage-name">{{ s.title }} — every {{ Math.round(s.interval_secs / 60) }} min</span>
              <code class="manage-query">next: {{ new Date(s.next_run_at).toLocaleString() }}<template v-if="s.last_result"> · last: {{ s.last_result }}</template></code>
              <button type="button" :disabled="schedulingBusy" @click="runNewsScheduleNowClick(s.id)">Run now</button>
              <button type="button" :disabled="schedulingBusy" @click="deleteNewsSchedule(s.id)">Delete</button>
            </li>
          </ul>
        </template>
      </div>
    </div>

    <div v-if="switchOpen" class="manage-backdrop" @click.self="switchOpen = false">
      <div class="manage-panel">
        <button class="manage-close" @click="switchOpen = false">✕</button>
        <h3>Switch library</h3>
        <ul v-if="recentLibraries.length" class="manage-list">
          <li v-for="path in recentLibraries" :key="path">
            <button type="button" :disabled="switching" @click="switchToRecent(path)">{{ path }}</button>
          </li>
        </ul>
        <p v-else class="news-hint">No other recently-opened libraries yet.</p>
        <div class="bulk-actions">
          <button type="button" class="read" :disabled="switching" @click="switchToOther">Browse for another…</button>
          <button type="button" :disabled="switching" @click="switchOpen = false">Close</button>
        </div>
        <p v-if="switchError" class="error">{{ switchError }}</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.library {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.5em;
  border-bottom: 1px solid #ddd;
  flex-wrap: wrap;
}
.sort-fields {
  display: flex;
  align-items: center;
  gap: 0.3em;
}
.sort-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.25em;
  background: #eef2fb;
  border-radius: 4px;
  padding: 0.2em 0.4em;
  font-size: 0.85em;
}
.sort-chip-remove {
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  font-size: 0.9em;
  line-height: 1;
}
.search {
  display: flex;
  flex: 1;
  min-width: 200px;
  gap: 0.25em;
}
.search input {
  flex: 1;
}
.body {
  flex: 1;
  display: flex;
  overflow: hidden;
}
.sidebar {
  width: 220px;
  flex-shrink: 0;
  border-right: 1px solid #ddd;
}
.grid-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: auto;
  padding: 0.5em;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  gap: 1em;
  flex: 1;
}
.card {
  background: none;
  border: none;
  cursor: pointer;
  text-align: left;
  padding: 0;
  font: inherit;
  position: relative;
}
.card.selected img {
  outline: 3px solid #2a6df4;
  outline-offset: -3px;
}
.card-checkbox {
  position: absolute;
  top: 0.3em;
  left: 0.3em;
  width: 1.1em;
  height: 1.1em;
  z-index: 1;
}
.card img {
  width: 100%;
  height: auto;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
}
.card-title {
  font-weight: 600;
  font-size: 0.85em;
  margin-top: 0.25em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-authors {
  font-size: 0.8em;
  color: #666;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pagination {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1em;
  padding: 0.75em 0;
}
.status,
.error {
  padding: 1em;
}
.error {
  color: #b00020;
}
.add-error,
.add-summary {
  padding: 0 0.5em;
}
.hidden-file-input {
  display: none;
}
.toolbar button.active {
  background: #2a6df4;
  color: #fff;
  border-color: #2a6df4;
}
.settings-link {
  padding: 0.35em 0.7em;
  border: 1px solid #ccc;
  border-radius: 4px;
  color: inherit;
  text-decoration: none;
}
.fts-enable {
  padding: 1em;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
  align-items: flex-start;
}
.fts-indexing {
  color: #a06a00;
}
.fts-results {
  list-style: none;
  margin: 0;
  padding: 0.5em;
  overflow: auto;
  flex: 1;
}
.fts-result {
  padding: 0.75em;
  border-bottom: 1px solid #eee;
  cursor: pointer;
}
.fts-result:hover {
  background: #f7f7f7;
}
.fts-result-header {
  display: flex;
  align-items: baseline;
  gap: 0.6em;
  flex-wrap: wrap;
}
.fts-result-title {
  font-weight: 600;
}
.fts-result-authors {
  color: #666;
  font-size: 0.9em;
}
.fts-result-formats {
  color: #999;
  font-size: 0.8em;
  text-transform: uppercase;
  margin-left: auto;
}
.fts-snippet {
  margin: 0.4em 0 0;
  font-size: 0.9em;
  color: #444;
}
.fts-snippet mark {
  background: #fff3a0;
  color: inherit;
  padding: 0 0.1em;
}
.bulk-panel {
  padding: 0.75em 1em;
  background: #f7f7f7;
  border-bottom: 1px solid #ddd;
}
.bulk-row {
  display: flex;
  flex-wrap: wrap;
  gap: 1em;
  align-items: flex-end;
}
.bulk-row label {
  display: flex;
  flex-direction: column;
  font-size: 0.85em;
  color: #555;
  gap: 0.2em;
}
.bulk-row input[type="text"],
.bulk-row input:not([type]) {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
.bulk-rating {
  flex-direction: row !important;
  align-items: center;
  gap: 0.4em !important;
}
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.bulk-errors {
  margin: 0.6em 0 0;
  padding: 0;
  list-style: none;
}
.manage-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}
.manage-panel {
  background: #fff;
  border-radius: 6px;
  padding: 1.5em;
  max-width: 560px;
  width: 90%;
  max-height: 85vh;
  overflow: auto;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1.25em;
}
.manage-close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: 1.1em;
  cursor: pointer;
}
.manage-panel h3 {
  margin: 0 0 0.5em;
}
.manage-list {
  list-style: none;
  margin: 0 0 0.75em;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.4em;
}
.manage-list li {
  display: flex;
  align-items: center;
  gap: 0.5em;
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.manage-query {
  color: #666;
  font-size: 0.85em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.manage-remove {
  color: #b00020;
}
.manage-form {
  display: flex;
  gap: 0.5em;
}
.manage-form input {
  flex: 1;
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
.news-panel {
  max-width: 480px;
}
.news-hint {
  color: #666;
  font-size: 0.9em;
  margin: 0;
}
.news-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-size: 0.85em;
  color: #555;
  margin-bottom: 0.75em;
}
.news-field input,
.news-field textarea {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
  resize: vertical;
}
.news-done {
  color: #2a7f2a;
  margin: 0.6em 0 0;
}
/* View toggle + column picker (issue 1.1). */
.view-toggle {
  display: inline-flex;
  gap: 0;
}
.view-toggle button {
  border-radius: 0;
}
.view-toggle button:first-child {
  border-top-left-radius: 4px;
  border-bottom-left-radius: 4px;
}
.view-toggle button:last-child {
  border-top-right-radius: 4px;
  border-bottom-right-radius: 4px;
}
.view-toggle button.active {
  font-weight: 600;
}
.column-list {
  list-style: none;
  margin: 0 0 1rem;
  padding: 0;
  max-height: 55vh;
  overflow-y: auto;
}
.column-row {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.15rem 0;
}
.column-row .field {
  flex: 1;
}
.hint.inline {
  font-size: 0.8em;
  opacity: 0.7;
}

</style>
