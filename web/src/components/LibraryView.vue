<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { alertDialog } from "./dialogs/confirm";
import ImportDuplicatesDialog from "./dialogs/ImportDuplicatesDialog.vue";
import NewLibraryDialog from "./dialogs/NewLibraryDialog.vue";
import SwitchLibraryDialog from "./dialogs/SwitchLibraryDialog.vue";
import CategoryBrowser from "./CategoryBrowser.vue";
import NoteEditor from "./NoteEditor.vue";
import BookDetailsPanel from "./BookDetailsPanel.vue";
import BookTable from "./BookTable.vue";
import { invalidateFieldValues } from "../library/fieldValues";
import BookGrid from "./BookGrid.vue";
import { createBookWindow } from "../library/bookWindow";
import { commitCellEdit, type CommitResult } from "../library/inlineEdit";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import ToolbarButton from "./ToolbarButton.vue";
import AppIcon from "./AppIcon.vue";
import MapperDialog from "./MapperDialog.vue";
import AnnotationsBrowser from "./AnnotationsBrowser.vue";
import PolishDialog from "./PolishDialog.vue";
import HelpDialog from "./HelpDialog.vue";
import TrashDialog from "./TrashDialog.vue";
import ImportFolderDialog from "./ImportFolderDialog.vue";
import { scanPromptHint, scanPromptText, shouldPrompt } from "../library/scanPrompt";
import { addBook, catalogDownloadUrl, fetchBooks, fetchFieldMetadata, fetchNaming, fetchSavedSearches, fetchVirtualLibraries, libraryExportUrl, search, setFields } from "../library/api";
import type { DuplicateBook, DuplicateGroup } from "../library/api";
import ResolveDialog from "./ResolveDialog.vue";
import CompareDuplicates from "./CompareDuplicates.vue";
import { createCompareController, MAX_BOOKS as COMPARE_MAX } from "../library/compare";
import { createDuplicatesStore } from "../library/duplicatesStore";
import { createResolveController } from "../library/resolve";
import { createOrganizeController } from "../library/organize";
import OrganizeDialog from "./OrganizeDialog.vue";
import NamingDialog from "./NamingDialog.vue";
import { createNamingController } from "../library/naming";
import { createMirrorController } from "../library/mirror";
import MirrorDialog from "./MirrorDialog.vue";
import { createRetitleController } from "../library/retitle";
import RetitleDialog from "./RetitleDialog.vue";
import { defaultMergeDestination } from "../library/merge";
import { parseSnippetSegments } from "../library/snippets";
import { similarBooksQuery } from "../library/query";
import { pathFromLibraryId, recentLibraryEntries } from "../library/recentLibraries";
import { readSavedChoice, readVlChoice, savedMenuItems, type SavedMenuId, type VlMenuId, vlMenuItems } from "../library/searchMenus";
import { applySortChoice, type SortMenuId, sortSummary as summariseSort } from "../library/sortMenu";
import { moveKeyBy, moveKeyTo, pickerOrder, resetColumnPrefs, toggleKey } from "../library/columnLayout";
import { clampWidth, columnsFor, DEFAULT_TABLE_PREFS, resolveColumns, TABLE_PREFS_PROFILE, type BookColumn, type LibraryViewMode, type TablePrefs } from "../library/columns";
import { actionEnabled, contextMenuEntries, refusalText, LIBRARY_ACTIONS, toolbarActionEnabled, TOOLBAR_LAYOUT, type ToolbarEntry, type ToolbarItem, toolbarOverflowEntries, toolbarOverflowIds, visibleToolbarActions, type ActionContext, type LibraryAction, type LibraryActionId } from "../library/actions";
import { syncDesktopMenu } from "../library/desktopMenu";
import { buildMenuSpec } from "../library/actions";
import { addChosenDuplicates, describeImport, importFiles, type PendingDuplicate } from "../library/importFiles";
import { activeFilters, classifyEmpty, type ActiveFilter } from "../library/emptyState";
import { draftCount } from "../library/metadataForm";
import { trashBooksFlow } from "../library/deleteFlow";
import DeleteConfirm from "./DeleteConfirm.vue";
import UndoToast from "./UndoToast.vue";
import { isTauri, tauriInvoke } from "../tauri";
import { humanError } from "../library/errorText";
import { shortcutHint } from "../library/shortcuts";
import { DEFAULT_LIBRARY_PREFS, DEFAULT_TOOLBAR_PREFS, fetchProfile, LIBRARY_PREFS_PROFILE, saveProfile, TOOLBAR_PREFS_PROFILE, type LibraryPrefs, type ToolbarPrefs } from "../settings/api";
import type { FieldMetaEntry } from "../library/types";
import ManageListsDialog from "./dialogs/ManageListsDialog.vue";
import ColumnPickerDialog from "./dialogs/ColumnPickerDialog.vue";
import CustomColumnsDialog from "./dialogs/CustomColumnsDialog.vue";
import CheckLibraryDialog from "./dialogs/CheckLibraryDialog.vue";
import DuplicatesDialog from "./dialogs/DuplicatesDialog.vue";
import MergeBooksDialog from "./dialogs/MergeBooksDialog.vue";
import RenameFilesDialog from "./dialogs/RenameFilesDialog.vue";
import CopyToLibraryDialog from "./dialogs/CopyToLibraryDialog.vue";
import EmbedMetadataDialog from "./dialogs/EmbedMetadataDialog.vue";
import SaveToDiskDialog from "./dialogs/SaveToDiskDialog.vue";
import NewsDialog from "./dialogs/NewsDialog.vue";
import BulkEditPanel from "./BulkEditPanel.vue";
import { useBulkEdit } from "../library/useBulkEdit";
import { useFtsSearch } from "../library/useFtsSearch";
import { useLibraryInfo } from "../library/useLibraryInfo";
import { useLibraryKeyboard } from "../library/useLibraryKeyboard";
import { usePanelLayout } from "../library/usePanelLayout";
import { useRowColoring } from "../library/useRowColoring";
import { useSelection } from "../library/useSelection";
import { conventionFileTemplate } from "../library/renameFiles";

// Real, persisted default (issue #721) -- overwritten by
// loadLibraryPrefs() below once its fetch resolves; starts at the
// same value DEFAULT_LIBRARY_PREFS uses so there's no visible flash
// for a user with no saved preferences yet.
// Marks (see the marks section below) feed the list's id source, so they
// are declared before the first search runs.
const markedIds = ref<Set<number>>(new Set());
const showMarkedOnly = ref(false);

const duplicateDefault = ref<LibraryPrefs["duplicateDefault"]>(DEFAULT_LIBRARY_PREFS.duplicateDefault);

const queryText = ref("");
const activeQuery = ref(""); // committed query -- what's actually sent, vs. the input box's live text
const sort = ref(DEFAULT_LIBRARY_PREFS.sort);
const sortOrder = ref<"asc" | "desc">(DEFAULT_LIBRARY_PREFS.sortOrder);
const vl = ref("");
/** Every book the library has, whatever the current search (for "0 of 61 books"); null until known. */
const libraryTotal = ref<number | null>(null);

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

/**
 * The Sort dropdown's contents.
 *
 * Replaces two `<select>`s plus a row of removable chips. The
 * underlying model is unchanged -- `sort` is still one comma-joined
 * string -- this is purely a better way to present it. calibre does
 * the same (`gui2/layout.py`, a `sort.png` menu button), and the
 * select-plus-chips version was the most form-like control in the
 * window.
 *
 * Ids are namespaced because one flat menu carries three different
 * kinds of choice: which field leads, which direction, and which extra
 * field to append.
 */
const sortMenuEntries = computed<ContextMenuEntry<SortMenuId>[]>(() => {
  const entries: ContextMenuEntry<SortMenuId>[] = [];

  for (const [key, label] of sortableFields.value) {
    entries.push({ id: `field:${key}`, label, enabled: true, checked: primarySort.value === key });
  }

  entries.push({ id: "dir:asc", label: "Ascending", enabled: true, checked: sortOrder.value === "asc", startsGroup: true });
  entries.push({ id: "dir:desc", label: "Descending", enabled: true, checked: sortOrder.value === "desc" });

  // Only offered once a secondary sort exists, so the common case --
  // one field, one direction -- stays a short menu.
  for (const [i, field] of secondarySortFields.value.entries()) {
    entries.push({
      id: `drop:${field}`,
      label: `Remove “then by ${fieldLabel(field)}”`,
      enabled: true,
      startsGroup: i === 0,
    });
  }

  for (const [i, [key, label]] of availableExtraSortFields.value.entries()) {
    entries.push({ id: `add:${key}`, label: `Then by ${label}`, enabled: true, startsGroup: i === 0 });
  }
  return entries;
});

/** One line summarising the sort, for the button and the status bar. */
const sortSummary = computed(() => summariseSort({ sort: sort.value, order: sortOrder.value }, fieldLabel));

function onSortMenuChoose(id: SortMenuId) {
  sortMenu.value = null;
  const next = applySortChoice({ sort: sort.value, order: sortOrder.value }, id);
  sort.value = next.sort;
  sortOrder.value = next.order;
  void runSearch();
  void rememberSort();
}

/**
 * Keeps the last sort the user chose (header click or Sort menu) as the library's
 * sort, the one a reload or a trip to Settings or the reader comes back to. It is
 * the same stored value as Settings > Default sort, so the two never disagree.
 */
async function rememberSort() {
  try {
    const stored = await fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE);
    await saveProfile(LIBRARY_PREFS_PROFILE, { ...DEFAULT_LIBRARY_PREFS, ...stored, sort: sort.value, sortOrder: sortOrder.value });
  } catch (e) {
    console.error("failed to save the sort order", e);
  }
}

/**
 * Virtual-library menu, replacing a `<select>`.
 *
 * A select cannot manage its own options, so creating or deleting a
 * virtual library lived in a separate "Manage lists" dialog with no
 * relationship to the control that applies one. The menu carries both.
 */
const vlMenuEntries = computed(() => vlMenuItems(Object.keys(virtualLibraries.value), vl.value));

const vlMenu = ref<{ x: number; y: number } | null>(null);

function onVlMenuChoose(id: VlMenuId) {
  vlMenu.value = null;
  const choice = readVlChoice(id);
  if (choice.kind === "manage") manageOpen.value = true;
  else vl.value = choice.name;
}

/** The label the virtual-library button wears: the active one, or all. */
const vlLabel = computed(() => vl.value || "All books");

/**
 * Saved-search menu, also replacing a `<select>`.
 *
 * The select had a `disabled selected` placeholder option as its
 * label, which is the standard workaround for a select that acts like
 * a menu -- and a sign it should have been a menu.
 */
const savedMenuEntries = computed(() => savedMenuItems(Object.keys(savedSearches.value)));

const savedMenu = ref<{ x: number; y: number } | null>(null);

function onSavedMenuChoose(id: SavedMenuId) {
  savedMenu.value = null;
  const choice = readSavedChoice(id);
  if (choice.kind === "manage") {
    manageOpen.value = true;
    return;
  }
  const query = savedSearches.value[choice.name];
  if (query !== undefined) applySavedSearch(query);
}

const sortMenu = ref<{ x: number; y: number } | null>(null);

function openSortMenu(anchorRect: { x: number; y: number }) {
  sortMenu.value = anchorRect;
}
// The library list is one continuous, virtualised list. `win` holds the
// ordered id of every matching book plus a lazy cache of the summaries
// the viewport has needed (library/bookWindow.ts).
const ALL_ROWS = 100_000_000;

async function fetchIdList() {
  if (showMarkedOnly.value) {
    const ids = [...markedIds.value];
    return { ids, total: ids.length };
  }
  const result = await search({ query: activeQuery.value, num: ALL_ROWS, offset: 0, sort: sort.value, sortOrder: sortOrder.value, vl: vl.value });
  return { ids: result.book_ids, total: result.total_num };
}

const win = createBookWindow({ fetchIds: fetchIdList, fetchBooks });
const totalNum = win.total;
const loading = ref(false);
const error = ref<string | null>(null);
const selectedBookId = ref<number | null>(null);
const cacheBust = ref(0);
const tableRef = ref<InstanceType<typeof BookTable> | null>(null);
const gridRef = ref<InstanceType<typeof BookGrid> | null>(null);

const addInput = ref<HTMLInputElement | null>(null);
const adding = ref(false);
const addError = ref<string | null>(null);

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
  void runSearch();
  void rememberSort();
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
// The panels beside the list: whether they show, how wide, and the splitters
// that resize them (../library/usePanelLayout.ts).
const { sidebarWidth, detailsWidth, startResize, narrow, panelShown, setPanel, togglePanel } = usePanelLayout();
const sidebarShown = computed(() => panelShown("sidebar"));
const detailsShown = computed(() => panelShown("details"));

const columnPickerOpen = ref(false);

function setColumnKeys(columns: string[]) {
  tablePrefs.value = { ...tablePrefs.value, columns };
  void saveTablePrefs();
}

function toggleColumn(key: string) {
  setColumnKeys(toggleKey(tablePrefs.value.columns, key));
}

function moveColumn(key: string, delta: number) {
  setColumnKeys(moveKeyBy(tablePrefs.value.columns, key, delta));
}

/** A header was dragged in the table: reorder what is actually shown. */
function onColumnReorder(next: { key: string; index: number }) {
  setColumnKeys(moveKeyTo(visibleColumns.value.map((c) => c.key), next.key, next.index));
}

/** The picker's rows: shown columns in order, then the hidden ones. */
const pickerColumns = computed(() => pickerOrder(availableColumns.value, tablePrefs.value.columns));

/** A row of the column picker was dropped on another shown row. */
function onPickerReorder(from: string, onto: string) {
  const keys = tablePrefs.value.columns;
  if (from === onto || !keys.includes(from) || !keys.includes(onto)) return;
  setColumnKeys(moveKeyTo(keys, from, keys.indexOf(onto)));
}

function resetColumns() {
  tablePrefs.value = resetColumnPrefs(tablePrefs.value);
  void saveTablePrefs();
}

// The dialogs below are components (../components/dialogs): each owns what it
// edits and reports what changed, and this view only says which are open.
const manageOpen = ref(false);

function onVirtualLibraryDeleted(name: string) {
  if (vl.value === name) vl.value = "";
}

const columnsOpen = ref(false);

// A custom column made (or removed) in the Custom columns dialog must show up in
// the column picker without a reload: re-read the field metadata when it closes.
function closeColumns() {
  columnsOpen.value = false;
  void loadMetadata();
}

/** A column was added or removed: the category browser and the details follow without a reload. */
function onColumnsChanged() {
  categoriesRefresh.value++;
  cacheBust.value++;
}

const checkLibraryOpen = ref(false);

// Item notes (issue #732) -- see NoteEditor.vue's own doc.
const noteTarget = ref<{ field: string; itemName: string } | null>(null);

function openNote(field: string, itemName: string) {
  noteTarget.value = { field, itemName };
}

function applySavedSearch(query: string) {
  queryText.value = query;
  activeQuery.value = query;
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

const newsOpen = ref(false);

/**
 * Re-run the current query after something changed (an edit, delete,
 * import, resolve). The scroll position and the loaded rows are kept:
 * rows already on screen stay until their fresh copy arrives, so
 * nothing flashes.
 */
async function runSearch() {
  loading.value = true;
  error.value = null;
  try {
    // Colours are re-evaluated for the rows on screen; the old ones stay until replaced.
    coloring.forgetAll();
    await win.refresh();
    error.value = win.error.value ? humanError(win.error.value) : null;
    pruneSelection();
  } finally {
    loading.value = false;
  }
}

/** A different query, sort or virtual library: start again from the top. */
async function resetSearch() {
  loading.value = true;
  error.value = null;
  try {
    await win.reset();
    error.value = win.error.value ? humanError(win.error.value) : null;
    // The selection must not outlive the results it was made in: a delete
    // would count books the new search hides.
    pruneSelection();
    // Nor may the open book: the details panel would keep Save, Delete and Read
    // live on a book no row shows. (A failed load shows nothing either way, so
    // it keeps what was open.)
    if (!win.error.value && selectedBookId.value !== null && win.indexOf(selectedBookId.value) < 0) selectedBookId.value = null;
    void refreshLibraryTotal();
  } finally {
    loading.value = false;
  }
}

/**
 * How many books the whole library holds, for the status bar's "0 of 61 books".
 * With no search or filter the list's own total is the answer; otherwise it is
 * asked once (one id, not the list).
 */
async function refreshLibraryTotal() {
  const filtered = activeFilters({ query: activeQuery.value, virtualLibrary: vl.value, showMarkedOnly: showMarkedOnly.value }).length > 0;
  if (!filtered) {
    libraryTotal.value = totalNum.value;
    return;
  }
  try {
    libraryTotal.value = (await search({ query: "", num: 1, offset: 0, sort: "id", sortOrder: "asc", vl: "" })).total_num;
  } catch {
    libraryTotal.value = null;
  }
}

/** After a refresh, selected books that no longer match are dropped from the selection. */
function pruneSelection() {
  if (selectedIds.value.size === 0) return;
  const next = new Set<number>();
  for (const id of selectedIds.value) if (win.indexOf(id) >= 0) next.add(id);
  if (next.size !== selectedIds.value.size) selectedIds.value = next;
}
// Full-text search (../library/useFtsSearch.ts). Declared before the immediate
// watcher below reads `ftsMode`: a `const` read before its line throws
// (temporal dead zone) and blanked the whole view.
const { ftsMode, ftsLoading, ftsError, ftsNotEnabled, ftsEnabling, ftsIndexing, ftsResults, runFtsSearch, enableFts } = useFtsSearch(activeQuery);

// `ftsMode` is a dependency: the list was not touched while full-text results
// were showing, so leaving that mode must load it again (it may be stale or
// have been built for a query that has since changed).
watch([activeQuery, sort, sortOrder, vl, ftsMode], () => {
  if (!ftsMode.value) void resetSearch();
}, { immediate: true });

function snippetSegments(text: string) {
  return parseSnippetSegments(text);
}

function submitSearch() {
  activeQuery.value = queryText.value;
}

function onCategorySelect(categoryQuery: string) {
  queryText.value = categoryQuery;
  activeQuery.value = categoryQuery;
}

/** Bumped to make the category browser (folders and categories) reload. */
const categoriesRefresh = ref(0);

/**
 * Something changed which books exist, their fields, or their folders: bring
 * every view that derives from the library up to date. Folder tree and
 * category counts (`categoriesRefresh`), the autocomplete values, the cover
 * cache and the list itself all go stale together, so every mutating flow
 * (organize, import, merge, trash restore, author mapping, category rename,
 * delete...) calls this one function instead of remembering a subset.
 */
function libraryChanged(): Promise<void> {
  categoriesRefresh.value++;
  invalidateFieldValues();
  cacheBust.value++;
  void refreshChangeLogStatus();
  return ftsMode.value ? runFtsSearch() : runSearch();
}

function onDetailsUpdated() {
  void libraryChanged();
}

/** Books were moved or a folder renamed: refresh the list in place (position kept). */
function onFoldersChanged() {
  void libraryChanged();
}

function onCategoryRenamed() {
  void libraryChanged();
}

function onDetailsDeleted(bookId: number) {
  // Only if it is still the open book: the selection may have moved on while the delete ran.
  if (selectedBookId.value === bookId) selectedBookId.value = null;
  void libraryChanged();
}

// Which books are selected (../library/useSelection.ts) and the bulk edit panel
// that acts on them (../library/useBulkEdit.ts).
const { selectMode, selectedIds, scope, selectOne, toggleSelected, rangeSelect, selectAll, onCardClick, onNavigate } = useSelection({ win, selectedBookId, viewMode, tableRef, gridRef });
const bulk = useBulkEdit({ selectedIds, selectMode, fieldMetadata, win, onChanged: libraryChanged });

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) {
    selectedIds.value = new Set();
    bulk.open = false;
  }
}

// Which rows the views are showing: load exactly those (plus overscan).
function onRange(range: { first: number; last: number }) {
  renderedRange.value = range;
  win.setRange(range.first, range.last);
}

// In-place edits from the table: applied to the row at once, saved with
// the same set-fields call the details panel uses, rolled back on failure.
// The row keeps its place even if the edit changes its sort key -- the
// list is only re-sorted by the next refresh or query.
const detailsRefresh = ref(0);

async function commitEdit(bookId: number, column: BookColumn, raw: string): Promise<CommitResult> {
  const result = await commitCellEdit({ target: win, bookId, column, raw, save: setFields });
  if (result.status === "saved") {
    invalidateFieldValues();
    if (selectedBookId.value === bookId) detailsRefresh.value++;
    coloring.forget(bookId);
    void coloring.apply();
  }
  return result;
}

// Row colouring rules (../library/useRowColoring.ts).
const coloring = useRowColoring(win);
const { rowColors, renderedRange } = coloring;

// Dialogs that act on the books the view acts on: the selection, else the open book.
const renameScope = scope.ids;
const saveToDiskOpen = ref(false);
const copyOpen = ref(false);
const embedOpen = ref(false);
const renameOpen = ref(false);

function openRenameFiles() {
  if (renameScope.value.length > 0) renameOpen.value = true;
}

/**
 * Opens the library's own folder in the system file browser (#886).
 *
 * The Rust side resolves the path from its own state rather than taking
 * one from here -- see `reveal_library_folder` for why a command that
 * opened any path it was handed would be the wrong shape.
 */
async function revealLibraryFolder() {
  try {
    await tauriInvoke<void>("reveal_library_folder");
  } catch (e) {
    error.value = humanError(e);
  }
}

// Find duplicates: the server scans the whole library once (a job, with
// progress), keeps the result, and the panel pages through it. The state and
// the rules are in ../library/duplicatesStore.ts and ../library/resolve.ts.
const duplicatesOpen = ref(false);
const dupStore = createDuplicatesStore();
const resolveController = createResolveController();
const resolveOpen = ref(false);
// Compare copies side by side (from a group card, or a selection of 2 to 8
// books): rules and state in ../library/compare.ts.
const compareController = createCompareController(undefined, {
  libraryChanged: () => void onDuplicatesChanged(),
  groupsChanged: () => dupStore.refill(),
});
function openCompareForGroup(group: DuplicateGroup) {
  void compareController.open(
    group.books.map((b) => b.book_id),
    { group },
  );
}
function openCompareForSelection() {
  const ids = [...selectedIds.value];
  if (ids.length < 2) return;
  if (ids.length > COMPARE_MAX) {
    void alertDialog(`Compare takes 2 to ${COMPARE_MAX} books at once (${ids.length} are selected).`, { title: "Compare" });
    return;
  }
  void compareController.open(ids);
}
const organizeController = createOrganizeController();
const organizeOpen = ref(false);
/** Organize files: the selected books when there are any (select mode), else the open book is not assumed. */
const organizeSelection = computed(() => scope.ids.value);
async function openOrganize(prefill?: { template: string; scope: "selected" | "all" }) {
  organizeController.open(organizeSelection.value.length);
  if (prefill) {
    organizeController.state.draft.template = prefill.template;
    organizeController.state.draft.scope = prefill.scope === "selected" && organizeSelection.value.length > 0 ? "selected" : "all";
  } else {
    // The library's naming convention is the default template; the dialog can still override it.
    try {
      organizeController.state.draft.template = (await fetchNaming()).combined;
    } catch {
      // The built-in default stays.
    }
  }
  organizeOpen.value = true;
}

// File naming: the library's folder and file naming convention.
const namingController = createNamingController();
const namingOpen = ref(false);
const namingSampleIds = computed(() => scope.ids.value.slice(0, 5));
function openNaming() {
  namingOpen.value = true;
}
function applyNaming(template: string, scope: "selected" | "all") {
  namingOpen.value = false;
  void openOrganize({ template, scope });
}
// Export a copy: a travel copy or a backup mirror to another folder or drive.
// The scope follows the selection like Organize; nothing in the library changes.
const mirrorController = createMirrorController();
const mirrorOpen = ref(false);
function openMirror() {
  mirrorController.open(organizeSelection.value.length);
  mirrorOpen.value = true;
}
// Fix titles: find junk titles, suggest clean ones, set / embed / rename in one
// run (undoable). From the Library menu it looks at every book; from the
// selection and the context menu, at the selected books.
const retitleController = createRetitleController();
const retitleOpen = ref(false);
const retitleSelection = scope.ids;
function openRetitle(scope: "all" | "selected") {
  if (scope === "selected" && retitleSelection.value.length === 0) return;
  retitleController.open(scope);
  retitleOpen.value = true;
  // Its rename template starts from the library's naming convention.
  void conventionFileTemplate().then((t) => {
    if (t && retitleController.state.rows.length === 0) retitleController.state.renameTemplate = t;
  });
}
async function onRetitleChanged() {
  await libraryChanged();
}

// Opens the panel on the server's last scan, or scans if there is none.
async function openDuplicates() {
  duplicatesOpen.value = true;
  await dupStore.open();
}

// "Resolve…" on the toolbar: the duplicates panel, with the rule dialog on top of it.
async function openResolveDuplicates() {
  await openDuplicates();
  resolveOpen.value = true;
}

// After books changed under the panel (a delete, a resolve run): refresh the library list.
async function onDuplicatesChanged() {
  await libraryChanged();
}

// ---------------------------------------------------------------
// Merge books: from a duplicate group ("Merge into…") or from a
// selection of two or more books (the "merge-books" action). The dialog is
// ../components/dialogs/MergeBooksDialog.vue; this is what it is opened with
// and what happens once it has merged.
// ---------------------------------------------------------------
const mergeOpen = ref(false);
const mergeBooksList = ref<DuplicateBook[]>([]);
const mergeDest = ref<number | null>(null);
const mergeDone = ref<string | null>(null);

// The duplicate group a merge was opened from, so it can leave the list afterwards.
const mergeGroupId = ref<string | null>(null);

function openMerge(books: DuplicateBook[], dest: number | null, groupId: string | null = null) {
  mergeGroupId.value = groupId;
  mergeBooksList.value = books;
  mergeDest.value = dest;
  mergeDone.value = null;
  mergeOpen.value = true;
}

function openMergeForGroup(group: DuplicateGroup) {
  openMerge(group.books, dupStore.keeperFor(group) ?? defaultMergeDestination(group.books), group.id ?? null);
}

// calibre's rule: the first selected book is the destination.
async function openMergeForSelection() {
  const ids = [...selectedIds.value];
  if (ids.length < 2) return;
  const books = await fetchBooks(ids);
  openMerge(
    books.map((b) => ({ book_id: b.id, title: b.title, authors: b.authors, formats: b.formats.length })),
    ids[0],
  );
}

async function onMerged(description: string) {
  mergeDone.value = description;
  mergeOpen.value = false;
  selectedIds.value = new Set();
  await libraryChanged();
  // The merged group leaves the list (and the server's copy of the scan) at
  // once; the rest of the page stays where it was.
  if (duplicatesOpen.value && mergeGroupId.value) await dupStore.resolved([mergeGroupId.value]);
  mergeGroupId.value = null;
}

const addSummary = ref<string | null>(null);

/** Duplicates found during an import, waiting for one decision. `checked` = add this one anyway. */
const importDupes = ref<(PendingDuplicate & { checked: boolean })[] | null>(null);
const importDupesBusy = ref(false);
/** What the import so far did, so the summary can be finished after the decision. */
let importSoFar: { total: number; added: number; skipped: number; errors: string[] } | null = null;

function finishImportSummary() {
  if (!importSoFar) return;
  const { total, added, skipped, errors } = importSoFar;
  if (errors.length > 0) addError.value = errors.join("; ");
  if (total > 1 || skipped > 0) addSummary.value = describeImport(total, added, skipped, errors.length);
  importSoFar = null;
}

async function onAddFileSelected(e: Event) {
  const input = e.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  if (files.length === 0) return;
  adding.value = true;
  addError.value = null;
  addSummary.value = null;
  const outcome = await importFiles(files, addBook, duplicateDefault.value);
  adding.value = false;
  if (addInput.value) addInput.value.value = "";
  importSoFar = { total: files.length, added: outcome.added, skipped: outcome.skipped, errors: outcome.errors };
  if (outcome.duplicates.length > 0) {
    // One list for the whole import, not one native confirm per file.
    importDupes.value = outcome.duplicates.map((d) => ({ ...d, checked: false }));
  } else finishImportSummary();
  await libraryChanged();
}

/** Resolves the duplicates list: add the checked ones (re-uploading them), skip the rest. */
async function resolveImportDupes(mode: "skip-all" | "add-all" | "checked") {
  const list = importDupes.value;
  if (!list || !importSoFar) return;
  const chosen = mode === "add-all" ? list : mode === "checked" ? list.filter((d) => d.checked) : [];
  importDupesBusy.value = true;
  try {
    const result = await addChosenDuplicates(chosen, addBook);
    importSoFar.added += result.added;
    importSoFar.errors.push(...result.errors);
    importSoFar.skipped += list.length - chosen.length;
  } finally {
    importDupesBusy.value = false;
  }
  importDupes.value = null;
  finishImportSummary();
  await libraryChanged();
}

interface AddFolderResult {
  added: number;
  duplicates: string[];
  errors: string[];
}

// "Add folder" is a job now, not a loop of uploads: the dialog picks the
// folder, the mode (one book per folder / several) and what to do about
// duplicates, copies recursively with progress and cancel, and reports
// every file it could not read instead of dropping it.
const importOpen = ref(false);

function addFolder() {
  addError.value = null;
  addSummary.value = null;
  importOpen.value = true;
}

async function onImported() {
  await libraryChanged();
  void loadLibraryInfo();
}

// What the library says about itself, and the banners and prompts that hang off
// it (../library/useLibraryInfo.ts).
const {
  libraryName, libraryPath, copyTargets, banner, bannerDismissed,
  changeLogWarning, changeLogRepairBusy, changeLogRepairError, refreshChangeLogStatus, repairChangeLogNow,
  loadLibraryInfo,
  untracked, scanPromptDismissed, promptScanBusy, promptScanProgress, promptScanSummary, promptScanError, scanFromPrompt, stopPromptScan, dismissScanPrompt, closeScanResult,
  interruptedJob, resumeBusy, resumeProgress, resumeError, loadInterruptedJob, resumeInterrupted,
  newLibraryOpen, newLibraryName, newLibraryError, creatingLibrary, openNewLibrary, createLibrary,
  switchOpen, recentLibraries, switching, switchError, loadRecentLibraries, openSwitchLibrary, switchToRecent, switchToOther,
} = useLibraryInfo({ libraryChanged: () => libraryChanged() });

const trashOpen = ref(false);

async function onTrashChanged() {
  await libraryChanged();
}

// Drag-and-drop add (#818). The drop itself is handled natively --
// Tauri's own drag-drop handling suppresses the webview's HTML5 drop
// events, so there is nothing for the page to listen for until the
// Rust side has already done the work and says so here.
//
// The page has to be told explicitly because it does not subscribe to
// lexmechanic_srv's websocket, so the `BooksAdded` change event the add
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
  void libraryChanged();
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

/** Book actions BookDetailsPanel knows how to perform. */
const PANEL_ACTIONS: LibraryActionId[] = ["read", "edit-metadata", "fetch-metadata", "convert", "tweak-book", "quick-view", "test-template", "send-email", "replace-cover", "cover-from-pdf", "open-externally", "open-book-folder", "unpack-book", "repack-book"];

const actionHandlers: Partial<Record<LibraryActionId, () => void>> = {
  "manage-lists": () => {
    manageOpen.value = true;
  },
  "custom-columns": () => {
    columnsOpen.value = true;
  },
  "check-library": () => {
    checkLibraryOpen.value = true;
  },
  "find-duplicates": () => openDuplicates(),
  "resolve-duplicates": () => void openResolveDuplicates(),
  "organize-files": () => void openOrganize(),
  "file-naming": () => openNaming(),
  "export-copy": () => openMirror(),
  "fix-titles": () => openRetitle("all"),
  "fix-titles-selected": () => openRetitle("selected"),
  "map-metadata": () => {
    mapperOpen.value = true;
  },
  "browse-annotations": () => {
    annotationsOpen.value = true;
  },
  "pick-random": () => pickRandomBook(),
  help: () => {
    helpOpen.value = true;
  },
  "mark-books": () => markSelection(),
  polish: () => {
    polishOpen.value = true;
  },
  "show-marked": () => toggleShowMarked(),
  "clear-marks": () => clearMarks(),
  "export-catalog": () => exportCatalog(),
  "export-library-archive": () => exportLibraryArchive(),
  "fetch-news": () => {
    newsOpen.value = true;
  },
  "add-books": () => addInput.value?.click(),
  "add-folder": () => addFolder(),
  trash: () => {
    trashOpen.value = true;
  },
  "switch-library": () => openSwitchLibrary(),
  "new-library": () => openNewLibrary(),
  "select-mode": () => toggleSelectMode(),
  "bulk-edit": () => {
    bulk.open = true;
  },
  "merge-books": () => void openMergeForSelection(),
  "compare-duplicates": () => openCompareForSelection(),
  "save-to-disk": () => {
    saveToDiskOpen.value = true;
  },
  "rename-files": () => void openRenameFiles(),
  "embed-metadata": () => {
    embedOpen.value = true;
  },
  "copy-to-library": () => {
    copyOpen.value = true;
  },
  "open-library-folder": () => void revealLibraryFolder(),
  // Book-scoped: the details panel performs these (it owns the controls),
  // so the toolbar and the keyboard reach it the way the context menu does.
  ...Object.fromEntries(PANEL_ACTIONS.map((id) => [id, () => runPanelAction(id)])),
  // With several books selected the metadata editor is the bulk editor.
  "edit-metadata": () => {
    if (actionContext.value.selectionCount > 1) bulk.open = true;
    else runPanelAction("edit-metadata");
  },
  "similar-books": () => {
    if (selectedBookId.value !== null) void showSimilarBooks(selectedBookId.value);
  },
  "delete-book": () => void deleteSelectedBooks(),
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
  "organize-files",
  "export-copy",
  "fix-titles",
  "map-metadata",
  "browse-annotations",
  "pick-random",
  "help",
  "mark-books",
  "export-catalog",
  "export-library-archive",
  "fetch-news",
  "select-mode",
  "bulk-edit",
  "merge-books",
  "compare-duplicates",
  "save-to-disk",
]);

const actionContext = computed<ActionContext>(() => ({
  selectionCount: selectedIds.value.size > 0 ? selectedIds.value.size : selectMode.value ? 0 : selectedBookId.value !== null ? 1 : 0,
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
    case "select-mode":
      return selectMode.value ? "Cancel selection" : action.label;
    case "edit-metadata":
      return scope.count.value > 1 ? `${action.label} (${scope.count.value})` : action.label;
    case "bulk-edit":
    case "save-to-disk":
    case "rename-files":
    case "fix-titles-selected":
    case "embed-metadata":
    case "copy-to-library":
      return `${action.label} (${scope.count.value})`;
    default:
      return action.label;
  }
}

function actionBusy(action: LibraryAction): boolean {
  if (action.id === "add-books") return adding.value;
  return false;
}

/**
 * Whether a toolbar entry should be clickable.
 *
 * Two separate reasons to be disabled: the action is mid-flight, or it
 * needs a selection there isn't one of. The second used to remove the
 * button entirely, which made the toolbar reflow as the selection
 * changed; it now stays put and greys out.
 */
function actionDisabled(action: LibraryAction): boolean {
  return actionBusy(action) || !toolbarActionEnabled(action, actionContext.value);
}

/** `select-mode` is a toggle, so it reflects its on state. */
function actionActive(action: LibraryAction): boolean {
  return action.id === "select-mode" && selectMode.value;
}

const toolbarActions = computed<LibraryAction[]>(() =>
  visibleToolbarActions(
    {
      handled: Object.keys(actionHandlers) as LibraryActionId[],
      hidden: toolbarPrefs.value.hidden,
      suppressed: ftsMode.value ? HIDDEN_IN_FTS_MODE : undefined,
      ctx: actionContext.value,
    },
    // The layout's slots and menus include book-scoped actions (Edit metadata, Remove).
    { includeBook: true },
  ),
);

/**
 * Opens the overflow beneath its button.
 *
 * Reuses `ContextMenu` -- the same component the right-click menu uses,
 * reading the same registry, so an action cannot appear in one place
 * and behave differently in the other.
 */
/**
 * The toolbar, resolved from `TOOLBAR_LAYOUT` against what this view
 * can actually do.
 *
 * A slot whose action this view does not handle, or that the platform
 * cannot offer, is dropped entirely -- but a slot that merely needs a
 * selection stays and renders disabled, so the row does not reflow as
 * the selection changes.
 */
const toolbarItems = computed(() => {
  const shown = new Set(toolbarActions.value.map((a) => a.id));
  return TOOLBAR_LAYOUT.filter((item) => {
    if (item.kind === "overflow") return toolbarOverflowIds().some((id) => shown.has(id));
    if (!("id" in item) || item.kind === "menu") return true;
    return shown.has(item.id);
  });
});

/** Resolves a slot id to its registry entry. */
function actionFor(id: LibraryActionId): LibraryAction | undefined {
  return LIBRARY_ACTIONS.find((a) => a.id === id);
}

function itemKey(item: ToolbarItem, index: number): string {
  return "id" in item ? `${item.kind}-${item.id}` : `${item.kind}-${index}`;
}

function itemHasMenu(item: ToolbarItem): boolean {
  return item.kind === "split" || item.kind === "menu" || item.kind === "overflow";
}

/**
 * The body was clicked. A pure menu slot opens its menu instead, beneath the
 * button: `anchor` is the button's own corner (ToolbarButton reports it), because
 * a menu opened at (0, 0) landed on top of the first buttons of the toolbar.
 */
function onItemRun(item: ToolbarItem, anchor: { x: number; y: number }, key = "") {
  if (item.kind === "action" || item.kind === "split") {
    runAction(item.id);
    return;
  }
  // A pure menu has no primary action, so clicking its body opens it.
  onItemMenu(item, anchor, key);
}

function onItemMenu(item: ToolbarItem, anchor: { x: number; y: number }, key = "") {
  if (item.kind === "split" || item.kind === "menu") void openSlotMenu(item.menu, anchor, key);
  else if (item.kind === "overflow") void openSlotMenu(toolbarOverflowEntries(), anchor, key);
}

/** Whether this slot's own dropdown is the one showing (for `aria-expanded`). */
function itemExpanded(key: string): boolean {
  return toolbarMenu.value !== null && toolbarMenu.value.owner === key;
}

/** What a screen reader calls a slot's arrow, where the label alone would not say. */
function itemMenuLabel(item: ToolbarItem): string | undefined {
  if (item.kind === "overflow") return "More actions";
  if (item.kind === "menu" && item.id === "library") return "Library menu";
  return undefined;
}

function itemLabel(item: ToolbarItem): string {
  if (item.kind === "overflow") return "More";
  if (item.kind === "separator" || item.kind === "spring") return "";
  // The library slot wears the open library's name: which library you
  // are in is state, and state belongs on a label.
  if (item.kind === "menu") return item.id === "library" ? libraryName.value || "Library" : item.label ?? "";
  if (item.kind === "split" && item.label) return item.label;
  const action = actionFor(item.id as LibraryActionId);
  return action ? actionLabel(action) : "";
}

function itemIcon(item: ToolbarItem): string | undefined {
  if (item.kind === "overflow") return "h-ellipsis";
  if (item.kind === "menu") return item.icon;
  if (!("id" in item)) return undefined;
  return actionFor(item.id as LibraryActionId)?.icon;
}

function itemDisabled(item: ToolbarItem): boolean {
  if (!("id" in item) || item.kind === "menu") return false;
  const action = actionFor(item.id as LibraryActionId);
  return action ? actionDisabled(action) : true;
}

function itemTitle(item: ToolbarItem): string | undefined {
  if (item.kind === "overflow") return "Everything else";
  if (item.kind === "menu") return libraryPath.value || undefined;
  if (!("id" in item)) return undefined;
  const action = actionFor(item.id as LibraryActionId);
  if (!action) return undefined;
  // The key comes from the keymap in force, not from text written beside the
  // action: that text said "Ctrl+A" and "E" for keys that did something else.
  const parts = [action.tooltip ?? action.label];
  const hint = shortcutHint(action.id, keymap.value);
  if (hint) parts.push(`(${hint})`);
  return parts.join(" ");
}

function itemActive(item: ToolbarItem): boolean {
  if (!("id" in item) || item.kind === "menu") return false;
  const action = actionFor(item.id as LibraryActionId);
  return action ? actionActive(action) : false;
}

/**
 * Expands a slot's menu into entries, dropping anything this view
 * cannot run and collapsing separators that end up adjacent.
 */
/**
 * A toolbar menu id: either a registry action, or a dynamic entry that
 * only exists while the menu is open.
 *
 * `lib:<path>` opens that library. The path is carried in the id
 * rather than looked up by index, because the recents list is refetched
 * whenever the menu opens and an index would go stale between the
 * render and the click.
 */
type SlotMenuId = LibraryActionId | `lib:${string}`;

function menuEntriesFor(entries: ToolbarEntry[]) {
  const handled = new Set(toolbarActions.value.map((a) => a.id));
  const out: { id: SlotMenuId; label: string; enabled: boolean; startsGroup?: boolean; accel?: string }[] = [];
  let pendingSeparator = false;

  const push = (entry: { id: SlotMenuId; label: string; enabled: boolean; accel?: string }) => {
    out.push({ ...entry, startsGroup: pendingSeparator });
    pendingSeparator = false;
  };

  for (const entry of entries) {
    if (entry === "-") {
      // Only becomes a real separator if something follows it.
      pendingSeparator = out.length > 0;
      continue;
    }

    if (entry === "$recent-libraries") {
      for (const recent of recentLibraryEntries(recentLibraries.value, libraryPath.value)) {
        push({ id: recent.id, label: recent.label, enabled: !switching.value });
      }
      continue;
    }

    // Other dynamic blocks are not wired yet; skipped rather than
    // rendered dead. Tracked as P2 in docs/UI_DESIGN.md §2.2.
    if (entry.startsWith("$")) continue;

    const id = entry as LibraryActionId;
    const action = actionFor(id);
    if (!action || !handled.has(id)) continue;
    const hint = shortcutHint(id, keymap.value);
    push({ id, label: actionLabel(action), enabled: !actionDisabled(action), ...(hint ? { accel: hint } : {}) });
  }
  return out;
}

/**
 * The open toolbar dropdown, if any.
 *
 * Holds its own entries rather than deriving them from an id, because
 * the overflow menu's contents are assembled from several slots at
 * once and do not correspond to any single one.
 */
const toolbarMenu = ref<{
  x: number;
  y: number;
  /** The slot that opened it, so only that slot's arrow reads as expanded. */
  owner: string;
  entries: { id: SlotMenuId; label: string; enabled: boolean; startsGroup?: boolean; accel?: string }[];
} | null>(null);

const toolbarMenuEntries = computed(() => toolbarMenu.value?.entries ?? []);

/** Opens a slot's dropdown, anchored beneath it. */
async function openSlotMenu(entries: ToolbarEntry[], anchor: { x: number; y: number }, owner = "") {
  // Refreshed on open rather than cached: another window may have
  // opened a library since this one last looked.
  if (entries.includes("$recent-libraries")) await loadRecentLibraries();
  const resolved = menuEntriesFor(entries);
  if (resolved.length === 0) return;
  toolbarMenu.value = { ...anchor, owner, entries: resolved };
}

function onToolbarMenuChoose(id: SlotMenuId) {
  toolbarMenu.value = null;
  const path = pathFromLibraryId(id);
  if (path !== null) {
    void switchToRecent(path);
    return;
  }
  runAction(id as LibraryActionId);
}

/** A short message under the toolbar that says why a click did nothing; it clears itself. */
const actionNotice = ref<string | null>(null);
let actionNoticeTimer: ReturnType<typeof setTimeout> | undefined;

function showActionNotice(text: string) {
  actionNotice.value = text;
  clearTimeout(actionNoticeTimer);
  actionNoticeTimer = setTimeout(() => (actionNotice.value = null), 5000);
}

/**
 * Runs an action. A click that cannot do anything says why instead of being
 * dropped silently; a key press (`quiet`) does not, since a stray key is not a
 * request.
 */
function runAction(id: LibraryActionId, quiet = false) {
  const handler = actionHandlers[id];
  if (!handler) return;
  const action = LIBRARY_ACTIONS.find((a) => a.id === id);
  if (action && !actionEnabled(action, actionContext.value)) {
    if (!quiet) showActionNotice(refusalText(action, actionContext.value));
    return;
  }
  handler();
}

// Keyboard shortcuts, the native menu and dropped books: window listeners that
// live only while the view is shown (../library/useLibraryKeyboard.ts).
const searchInput = ref<HTMLInputElement | null>(null);
const keyboard = useLibraryKeyboard({
  runAction: (id) => runAction(id, true),
  deleteSelectedBooks: () => void deleteSelectedBooks(),
  focusSearch: () => {
    searchInput.value?.focus();
    searchInput.value?.select();
  },
  toggleView: () => setViewMode(viewMode.value === "table" ? "grid" : "table"),
  contextMenuOpen: () => contextMenu.value !== null,
  onBooksDropped,
  resumed(stale) {
    void syncDesktopMenu(Object.keys(actionHandlers) as LibraryActionId[], actionContext.value);
    if (stale) void libraryChanged();
  },
});
const { keymap } = keyboard;
onMounted(() => {
  void loadLibraryInfo();
  void loadInterruptedJob();
});

// Keep the desktop app's native menu in step with what this view can
// currently do. Re-sent whenever enablement could have changed; a
// no-op in a plain browser tab.
// Watches a serialized key of the spec itself, not the context: the
// selection changes on every arrow-key step, but the spec (what is
// enabled) only rarely does, and each send is an IPC round trip that
// rebuilds the native menu.
const menuSpecKey = computed(() => {
  void ftsMode.value;
  void toolbarPrefs.value;
  return JSON.stringify(buildMenuSpec(Object.keys(actionHandlers) as LibraryActionId[], actionContext.value));
});
watch(
  menuSpecKey,
  () => {
    // A view kept alive behind the reader must not rewrite the native menu.
    if (!keyboard.isViewActive()) return;
    void syncDesktopMenu(Object.keys(actionHandlers) as LibraryActionId[], actionContext.value);
  },
  { immediate: true },
);

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

const contextEntries = computed(() =>
  contextMenuEntries([...PANEL_ACTIONS, "similar-books", "mark-books", "polish", "bulk-edit", "save-to-disk", "delete-book"], actionContext.value).map((entry) => {
    const hint = shortcutHint(entry.id, keymap.value);
    return hint ? { ...entry, accel: hint } : entry;
  }),
);

function openContextMenu(payload: { bookId: number; x: number; y: number }) {
  // Right-clicking a row that is not part of the current selection
  // acts on that row, matching every file manager: the click moves
  // the selection first, then the menu describes it.
  if (selectedIds.value.has(payload.bookId)) selectedBookId.value = payload.bookId;
  else if (!selectMode.value) selectOne(payload.bookId);
  else toggleSelected(payload.bookId);
  contextMenu.value = { x: payload.x, y: payload.y };
}

async function deleteSelectedBooks() {
  const ids = scope.ids.value;
  if (ids.length === 0) return;
  await win.ensureLoaded(ids).catch(() => []);
  const items = ids.map((id) => ({ id, title: win.getById(id)?.title ?? `Book ${id}` }));
  // The selection can hold books the current search no longer shows.
  const hiddenCount = ids.filter((id) => win.indexOf(id) < 0).length;
  try {
    const result = await trashBooksFlow(items, { hiddenCount, onRestored: libraryChanged });
    if (result.cancelled || !result.outcome) return;
    const gone = new Set(result.outcome.deleted);
    if (selectedBookId.value !== null && gone.has(selectedBookId.value)) selectedBookId.value = null;
    selectedIds.value = new Set([...selectedIds.value].filter((id) => !gone.has(id)));
    await libraryChanged();
    // A failed delete is shown where errors are, not swallowed. After the
    // refresh, which would otherwise clear it.
    if (result.failureText) error.value = result.failureText;
  } catch (e) {
    error.value = humanError(e);
  }
}

/** Hands a book action to the details panel, opening the panel if it is closed. */
function runPanelAction(id: LibraryActionId) {
  if (selectedBookId.value === null) return;
  setPanel("details", true);
  actionNonce += 1;
  pendingBookAction.value = { id, nonce: actionNonce };
}

function onContextChoose(id: LibraryActionId) {
  if (id === "similar-books") {
    if (selectedBookId.value !== null) void showSimilarBooks(selectedBookId.value);
    return;
  }
  if (id === "mark-books") {
    markSelection();
    return;
  }
  if (id === "delete-book") {
    void deleteSelectedBooks();
    return;
  }
  if (id === "bulk-edit" || id === "save-to-disk" || id === "edit-metadata") {
    runAction(id);
    return;
  }
  // Everything else belongs to the details panel.
  actionNonce += 1;
  pendingBookAction.value = { id, nonce: actionNonce };
}

// Author/tag mapping (#1.7). The engines -- metadata::author_mapper
// and ::tag_mapper -- were ported long ago with nothing calling them;
// this and POST /mapper/{preview,apply} are the callers.
const mapperOpen = ref(false);

/** Scope: the current selection, or the whole library when empty. */
const mapperScope = computed(() => [...selectedIds.value]);

async function onMapperApplied() {
  await libraryChanged();
}

// Library-wide annotation browser (#1.8).
const annotationsOpen = ref(false);

function openBookFromAnnotation(bookId: number) {
  annotationsOpen.value = false;
  selectedBookId.value = bookId;
}

// ---------------------------------------------------------------
// Marks, similar books, random (#1.5 / #1.9 / #1.16)
// ---------------------------------------------------------------
//
// Marks are what selection is not: they survive a new search, so
// books can be gathered across several queries and then acted on
// together. Session-only, matching upstream -- a mark is a working
// note, not metadata, and persisting it would make it something the
// user has to clean up.

function markSelection() {
  const ids = scope.ids.value;
  if (ids.length === 0) return;
  const next = new Set(markedIds.value);
  // Toggle as a group: if everything in the selection is already
  // marked, the user means to unmark it.
  const allMarked = ids.every((id) => next.has(id));
  for (const id of ids) {
    if (allMarked) next.delete(id);
    else next.add(id);
  }
  markedIds.value = next;
}

function clearMarks() {
  markedIds.value = new Set();
  if (showMarkedOnly.value) void toggleShowMarked();
}

/**
 * Shows exactly the marked books, by fetching them directly rather
 * than filtering the current page -- marks span searches, so most of
 * them are usually not in whatever result set is on screen.
 */
async function toggleShowMarked() {
  showMarkedOnly.value = !showMarkedOnly.value;
  // The list's id source reads `showMarkedOnly`: exactly the marked
  // ids, in place of a search.
  await resetSearch();
}

// What the list says when it has no rows: four different situations (see
// library/emptyState.ts), each with the one thing to do about it.
const emptyFilters = computed<ActiveFilter[]>(() => activeFilters({ query: activeQuery.value, virtualLibrary: vl.value, showMarkedOnly: showMarkedOnly.value }));
const emptyKind = computed(() =>
  classifyEmpty({ loading: loading.value, error: error.value, total: totalNum.value, query: activeQuery.value, virtualLibrary: vl.value, showMarkedOnly: showMarkedOnly.value }),
);

/** The list is not offered while it has nothing to say: an empty header over a message is clutter. */
const showList = computed(() => emptyKind.value !== "error" && emptyKind.value !== "empty-library" && emptyKind.value !== "no-results");

/** The library could not be loaded: nothing on screen is current, so nothing on screen is editable. */
const offline = computed(() => win.error.value !== null && emptyKind.value === "error");

/** The status bar's count: "61 books", or "0 of 61 books" when a search or filter hides some. */
const countText = computed(() => {
  if (offline.value) return "—";
  const n = totalNum.value;
  const noun = (count: number) => `book${count === 1 ? "" : "s"}`;
  const filtered = activeFilters({ query: activeQuery.value, virtualLibrary: vl.value, showMarkedOnly: showMarkedOnly.value }).length > 0;
  if (filtered && libraryTotal.value !== null && libraryTotal.value !== n) return `${n.toLocaleString()} of ${libraryTotal.value.toLocaleString()} books`;
  return `${n.toLocaleString()} ${noun(n)}`;
});

/**
 * "Skip to book list": from the first Tab stop straight to the current row, past the
 * toolbar's dozen buttons and the whole category tree.
 */
function skipToList() {
  const root = document.getElementById("book-list");
  const target = root?.querySelector<HTMLElement>('tr[tabindex="0"]') ?? root?.querySelector<HTMLElement>(".table-scroll, .book-card") ?? root;
  target?.focus({ preventScroll: true });
}

/** One click removes one filter. */
function clearFilter(id: ActiveFilter["id"]) {
  if (id === "search" || id === "folder") {
    queryText.value = "";
    activeQuery.value = "";
  } else if (id === "virtual-library") vl.value = "";
  else void toggleShowMarked();
}

function clearAllFilters() {
  queryText.value = "";
  activeQuery.value = "";
  vl.value = "";
  if (showMarkedOnly.value) void toggleShowMarked();
}

function retryLoad() {
  error.value = null;
  void resetSearch();
}

/** Similar books (#1.9): a plain search, using the existing engine. */
async function showSimilarBooks(bookId: number) {
  const book = win.getById(bookId) ?? (await fetchBooks([bookId]))[0];
  if (!book) return;
  const query = similarBooksQuery(
    { id: book.id, authors: book.authors, tags: book.tags, series: book.series, publisher: typeof book.publisher === "string" ? book.publisher : null },
    { authors: true, tags: true, series: true, publisher: true },
  );
  if (!query) {
    error.value = "That book has no authors, tags, series or publisher to find similar books by.";
    return;
  }
  if (showMarkedOnly.value) showMarkedOnly.value = false;
  selectedBookId.value = null;
  queryText.value = query;
  activeQuery.value = query;
  await resetSearch();
}

/** Random book (#1.16): opens one from the whole library. */
async function pickRandomBook() {
  try {
    // Ids only -- the point is to pick one, not to render a page.
    const all = await search({ query: "", num: ALL_ROWS, offset: 0, sort: "id", sortOrder: "asc", vl: vl.value });
    if (all.book_ids.length === 0) return;
    const id = all.book_ids[Math.floor(Math.random() * all.book_ids.length)];
    selectedBookId.value = id;
  } catch (e) {
    error.value = humanError(e);
  }
}

// Polish (#1.10). Acts on the selection, or the open book when
// nothing is explicitly selected.
const polishOpen = ref(false);

const polishScope = scope.ids;

async function onPolished() {
  await libraryChanged();
}

// In-app help (#1.17), generated from the registry and the live
// keymap so it cannot go stale.
const helpOpen = ref(false);
</script>

<template>
  <div class="library">
    <a class="skip-link" href="#book-list" @click.prevent="skipToList">Skip to book list</a>
    <header class="toolbar">
      <div class="toolbar-row toolbar-actions">
      <!--
        Rendered from TOOLBAR_LAYOUT (#817, then docs/UI_DESIGN.md §2.2).
        The registry is what the context menu, keyboard shortcuts and
        the desktop native menu all read from, so rendering the toolbar
        from it too is what keeps those four surfaces in agreement.
      -->
      <template v-for="(item, i) in toolbarItems" :key="itemKey(item, i)">
        <span v-if="item.kind === 'separator'" class="tb-sep" aria-hidden="true"></span>
        <span v-else-if="item.kind === 'spring'" class="toolbar-gap"></span>
        <ToolbarButton
          v-else
          :label="itemLabel(item)"
          :icon="itemIcon(item)"
          :has-menu="itemHasMenu(item)"
          :disabled="itemDisabled(item)"
          :active="itemActive(item)"
          :title="itemTitle(item)"
          :menu-label="itemMenuLabel(item)"
          :expanded="itemExpanded(itemKey(item, i))"
          :body-opens-menu="item.kind === 'menu' || item.kind === 'overflow'"
          @run="onItemRun(item, $event, itemKey(item, i))"
          @menu="onItemMenu(item, $event, itemKey(item, i))"
        />
      </template>

      <!--
        Marks need a visible home: "Show marked" and "Clear marks"
        are deliberately not toolbar actions (they would clutter it
        permanently), so they live in an indicator that only appears
        once something is actually marked.
      -->
      <span v-if="markedIds.size > 0" class="marks-indicator">
        <span class="marks-count">{{ markedIds.size }} marked</span>
        <button type="button" :class="{ active: showMarkedOnly }" :aria-pressed="showMarkedOnly" @click="toggleShowMarked">
          {{ showMarkedOnly ? "Show all" : "Show marked" }}
        </button>
        <button type="button" title="Clear all marks" @click="clearMarks">Clear</button>
      </span>
      <input ref="addInput" type="file" multiple class="hidden-file-input" @change="onAddFileSelected" />
      <router-link to="/settings" class="settings-link">Settings…</router-link>
      </div>
      <div class="toolbar-row toolbar-find">
      <!--
        Virtual library first, as calibre does: it scopes everything
        the search box then operates on, so it reads left to right.
      -->
      <ToolbarButton
        label=""
        icon="vl"
        compact
        has-menu
        :title="`Virtual library: ${vlLabel}`"
        menu-label="Virtual library options"
        :expanded="vlMenu !== null"
        body-opens-menu
        @run="vlMenu = $event"
        @menu="vlMenu = $event"
      />
      <span class="vl-name" :class="{ active: vl !== '' }">{{ vlLabel }}</span>
      <button v-if="vl !== ''" type="button" class="vl-clear" title="Show all books again" aria-label="Show all books again" @click="vl = ''">✕</button>

      <!--
        The search field owns its own mode. Full-text search used to be
        a separate labelled button beside the box, which described a
        property of the search as though it were an action.
      -->
      <form class="search" @submit.prevent="submitSearch">
        <button
          type="button"
          class="in-field"
          :class="{ active: ftsMode }"
          :aria-pressed="ftsMode"
          :title="ftsMode ? 'Searching inside books — click for metadata search' : 'Search inside book text'"
          :aria-label="ftsMode ? 'Search inside books (on)' : 'Search inside books'"
          @click="ftsMode = !ftsMode"
        >
          <AppIcon name="fts" :size="15" class="in-field-icon" />
        </button>
        <input ref="searchInput" v-model="queryText" type="search" :aria-label="ftsMode ? 'Search inside books' : 'Search library'" :placeholder="ftsMode ? 'Search book contents…' : 'Search…'" />
        <button v-if="queryText" type="button" class="in-field" title="Clear search" aria-label="Clear search" @click="queryText = ''; submitSearch()">✕</button>
        <button type="submit" class="search-go" title="Search" aria-label="Search">
          <AppIcon name="search" :size="15" class="in-field-icon" />
        </button>
      </form>

      <template v-if="!ftsMode">
        <!--
          One Sort button, not two selects and a row of chips. The
          model underneath is unchanged; this is presentation.
        -->
        <ToolbarButton
          label="Sort"
          icon="sort"
          compact
          has-menu
          :title="`Sorted by ${sortSummary}`"
          menu-label="Sort options"
          :expanded="sortMenu !== null"
          body-opens-menu
          @run="openSortMenu($event)"
          @menu="openSortMenu($event)"
        />
        <ToolbarButton
          label=""
          icon="folder_saved_search"
          compact
          has-menu
          title="Saved searches"
          menu-label="Saved searches"
          :expanded="savedMenu !== null"
          body-opens-menu
          @run="savedMenu = $event"
          @menu="savedMenu = $event"
        />
        <div class="view-toggle" role="group" aria-label="Library view">
          <button type="button" :class="{ active: viewMode === 'table' }" :aria-pressed="viewMode === 'table'" @click="setViewMode('table')">Table</button>
          <button type="button" :class="{ active: viewMode === 'grid' }" :aria-pressed="viewMode === 'grid'" @click="setViewMode('grid')">Grid</button>
        </div>
        <button v-if="viewMode === 'table'" type="button" @click="columnPickerOpen = true">Columns…</button>
      </template>

      </div>

    </header>

    <!--
      Where the library lives (#258). Advice only and dismissible: the
      library stays fully usable, and the banner links to nothing.
    -->
    <div v-if="banner && !bannerDismissed" class="storage-banner" :class="banner.level" role="status">
      <span class="storage-banner-text"><strong>{{ banner.title }}.</strong> {{ banner.detail }}</span>
      <button type="button" class="storage-banner-close" title="Dismiss" aria-label="Dismiss" @click="bannerDismissed = true">×</button>
    </div>

    <!-- The change log missed changes the database has: a rebuild from it would lose them. Repair appends them. -->
    <div v-if="changeLogWarning" class="storage-banner warning change-log-banner" role="alert">
      <span class="storage-banner-text">
        <strong>{{ changeLogWarning.text }}</strong>
        <template v-if="changeLogRepairError"> Repair failed: {{ changeLogRepairError }}</template>
        <template v-else> Repairing records them so the log is whole again.</template>
      </span>
      <button type="button" :disabled="changeLogRepairBusy" @click="repairChangeLogNow">{{ changeLogRepairBusy ? "Repairing…" : "Repair" }}</button>
    </div>

    <!--
      A folder somebody filled is not scanned just by being opened. Ask, say
      how many, and let the user decide: on a share a scan takes long.
    -->
    <div v-if="shouldPrompt(untracked, scanPromptDismissed) || promptScanBusy || promptScanSummary || promptScanError" class="storage-banner scan-prompt" role="status">
      <span class="storage-banner-text">
        <template v-if="promptScanBusy"><strong>Scanning.</strong> {{ promptScanProgress ?? "Starting…" }}</template>
        <template v-else-if="promptScanError"><strong>Scan failed.</strong> {{ promptScanError }}</template>
        <template v-else-if="promptScanSummary"><strong>Scan finished.</strong> {{ promptScanSummary }}</template>
        <template v-else-if="untracked">
          <strong>{{ scanPromptText(untracked) }}</strong> {{ scanPromptHint(untracked) }}
        </template>
      </span>
      <button v-if="promptScanBusy" type="button" @click="stopPromptScan">Stop</button>
      <template v-else-if="!promptScanSummary && !promptScanError">
        <button type="button" @click="scanFromPrompt">Scan now</button>
        <button type="button" @click="dismissScanPrompt">Not now</button>
      </template>
      <button v-else type="button" class="storage-banner-close" title="Dismiss" aria-label="Dismiss" @click="closeScanResult">×</button>
    </div>

    <div v-if="interruptedJob" class="storage-banner scan-prompt" role="status">
      <span class="storage-banner-text">
        <template v-if="resumeBusy"><strong>Resuming.</strong> {{ resumeProgress ?? "Starting…" }}</template>
        <template v-else-if="resumeError"><strong>Could not resume.</strong> {{ resumeError }}</template>
        <template v-else><strong>A {{ interruptedJob.kind.replace(/_/g, " ") }} job was interrupted.</strong> What it did is kept; it can carry on from there.</template>
      </span>
      <button v-if="!resumeBusy" type="button" @click="resumeInterrupted">Resume {{ interruptedJob.kind.replace(/_/g, " ") }}</button>
      <button v-if="!resumeBusy" type="button" @click="interruptedJob = null">Dismiss</button>
    </div>

    <p v-if="actionNotice" class="status action-notice" role="status">{{ actionNotice }}</p>
    <p v-if="addError" class="error add-error" role="alert">{{ addError }}</p>
    <p v-if="addSummary" class="status add-summary" role="status">{{ addSummary }}</p>

    <div class="body" :class="{ 'body-narrow': narrow }">
      <template v-if="sidebarShown">
        <CategoryBrowser class="sidebar" :class="{ 'sidebar-drawer': narrow }" :style="{ width: sidebarWidth + 'px' }" :selected-ids="renameScope" :refresh-key="categoriesRefresh" @select="onCategorySelect" @view-note="openNote" @renamed="onCategoryRenamed" @folders-changed="onFoldersChanged" />
        <div
          v-if="!narrow"
          class="splitter"
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize"
          @pointerdown="startResize('sidebar', $event)"
        ></div>
      </template>

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
            <li v-for="r in ftsResults" :key="r.bookId" class="fts-result" tabindex="0" :aria-label="`Open ${r.title}`" @click="selectedBookId = r.bookId" @keydown.enter.self.prevent="selectedBookId = r.bookId" @keydown.space.self.prevent="selectedBookId = r.bookId">
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

      <main v-else id="book-list" class="grid-area" aria-label="Books" tabindex="-1">
        <div v-if="emptyKind === 'error'" class="empty-state" data-testid="empty-error" role="alert">
          <p class="error">{{ error }}</p>
          <div class="empty-actions"><button type="button" class="primary" data-testid="empty-retry" @click="retryLoad">Retry</button></div>
        </div>
        <p v-else-if="emptyKind === 'loading'" class="status">Loading…</p>
        <div v-else-if="emptyKind === 'empty-library'" class="empty-state" data-testid="empty-library">
          <h3>This library has no books yet</h3>
          <p class="hint">Add book files, or a whole folder of them. Duplicates are found afterwards.</p>
          <div class="empty-actions">
            <button type="button" class="primary" data-testid="empty-add-books" @click="runAction('add-books')">Add books…</button>
            <button type="button" data-testid="empty-add-folder" @click="runAction('add-folder')">Add folder…</button>
          </div>
        </div>
        <div v-else-if="emptyKind === 'no-results'" class="empty-state" data-testid="empty-results" role="status">
          <h3>No books match your search</h3>
          <p class="hint">{{ emptyFilters.length > 1 ? "These filters are hiding every book:" : "This is hiding every book:" }}</p>
          <ul class="filter-chips">
            <li v-for="f in emptyFilters" :key="f.id" class="filter-chip" :data-filter="f.id">
              <span>{{ f.label }}</span>
              <button type="button" :aria-label="`Clear ${f.label}`" :title="`Clear ${f.label}`" @click="clearFilter(f.id)">✕</button>
            </li>
          </ul>
          <div class="empty-actions"><button type="button" class="primary" data-testid="empty-clear-all" @click="clearAllFilters">{{ emptyFilters.length > 1 ? "Clear all filters" : "Clear search" }}</button></div>
        </div>

        <BookTable
          v-if="viewMode === 'table' && showList"
          ref="tableRef"
          :ids="win.ids.value"
          :get-book="win.getById"
          :columns="visibleColumns"
          :select-mode="selectMode"
          :selected-ids="selectedIds"
          :selected-book-id="selectedBookId"
          :sort="sort"
          :sort-order="sortOrder"
          :reset-key="win.resetCount.value"
          :commit="commitEdit"
          @select="selectOne"
          @toggle-selected="toggleSelected"
          @range-select="rangeSelect"
          @select-all="selectAll"
          @sort-by="onTableSort"
          @resize="onColumnResize"
          @reorder="onColumnReorder"
          @context-menu="openContextMenu"
          @range="onRange"
          @navigate="onNavigate"
          :row-colors="rowColors"
        />

        <BookGrid
          v-else-if="viewMode === 'grid' && showList"
          ref="gridRef"
          :ids="win.ids.value"
          :get-book="win.getById"
          :select-mode="selectMode"
          :selected-ids="selectedIds"
          :selected-book-id="selectedBookId"
          :marked-ids="markedIds"
          :cache-bust="cacheBust"
          :reset-key="win.resetCount.value"
          @select="onCardClick"
          @toggle-selected="(id: number, e: MouseEvent) => (e.shiftKey ? rangeSelect(id) : toggleSelected(id))"
          @context-menu="openContextMenu"
          @range="onRange"
          @navigate="onNavigate"
        />
      </main>

      <!--
        Bulk edit takes the details panel's place instead of opening above the
        whole window, which pushed the sidebar and the list 140px down and back up.
      -->
      <aside v-if="bulk.open" class="details-column bulk-dock" :class="{ 'details-drawer': narrow }" aria-label="Bulk edit" :style="narrow ? undefined : { width: Math.max(detailsWidth, 400) + 'px' }">
        <BulkEditPanel :bulk="bulk" :count="selectedIds.size" />
      </aside>

      <!--
        Docked, not overlaid: calibre keeps details beside the list so
        arrowing down the rows updates them continuously. As a modal it
        covered the very list it describes.
      -->
      <template v-if="detailsShown && selectedBookId !== null && !offline && !bulk.open">
        <div
          v-if="!narrow"
          class="splitter"
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize"
          @pointerdown="startResize('details', $event)"
        ></div>
        <aside class="details-column" :class="{ 'details-drawer': narrow }" aria-label="Book details" :style="narrow ? undefined : { width: detailsWidth + 'px' }">
          <BookDetailsPanel
            :book-id="selectedBookId"
            :pending-action="pendingBookAction"
            :refresh-key="detailsRefresh"
            docked
            @close="setPanel('details', false)"
            @updated="onDetailsUpdated"
            @deleted="onDetailsDeleted"
            @open-book="selectedBookId = $event"
            @step-book="onNavigate($event > 0 ? 'ArrowDown' : 'ArrowUp')"
          />
        </aside>
      </template>
    </div>

    <!--
      Status bar: calibre's is always-on and is where the counts live.
      The panel toggles sit here rather than in the toolbar because
      they are view state, not actions on books.
    -->
    <footer class="status-bar">
      <!--
        Which library is open. Before this nothing on screen said, and
        `/ajax/library-info` reported a hardcoded name with no path, so
        two libraries were indistinguishable once you were inside one.
      -->
      <button
        v-if="libraryName"
        type="button"
        class="status-library"
        :title="libraryPath ? `${libraryPath}\nClick to switch library` : 'Click to switch library'"
        @click="openSwitchLibrary"
      >
        📚 {{ libraryName }}
      </button>
      <span class="status-count" role="status" aria-live="polite">{{ countText }}</span>
      <span v-if="totalNum > 0 && renderedRange.last > renderedRange.first" class="status-sort status-rows" title="Rows currently loaded around the viewport">rows {{ (renderedRange.first + 1).toLocaleString() }}–{{ renderedRange.last.toLocaleString() }}</span>
      <button v-if="selectMode && totalNum > 0" type="button" class="status-toggle" title="Select every matching book" @click="selectAll">Select all {{ totalNum.toLocaleString() }}</button>
      <span v-if="draftCount > 0" class="status-sel status-draft" data-testid="draft-count" title="Edits in the details panel that were not saved">{{ draftCount }} unsaved {{ draftCount === 1 ? "draft" : "drafts" }}</span>
      <span v-if="selectedIds.size > 0" class="status-sel">{{ selectedIds.size.toLocaleString() }} selected</span>
      <span v-if="markedIds.size > 0" class="status-sel">{{ markedIds.size }} marked</span>
      <span class="status-spacer"></span>
      <span class="status-sort">Sorted by {{ sortSummary }}</span>
      <button type="button" class="status-toggle" :class="{ active: sidebarShown }" :aria-pressed="sidebarShown" title="Show or hide the category browser" @click="togglePanel('sidebar')">Categories</button>
      <button type="button" class="status-toggle" :class="{ active: detailsShown }" :aria-pressed="detailsShown" title="Show or hide book details" @click="togglePanel('details')">Details</button>
    </footer>

    <ImportDuplicatesDialog v-if="importDupes" :duplicates="importDupes" :busy="importDupesBusy" @resolve="resolveImportDupes" />

    <DeleteConfirm />
    <UndoToast />

    <HelpDialog v-if="helpOpen" :keymap="keymap" @close="helpOpen = false" />

    <TrashDialog v-if="trashOpen" @close="trashOpen = false" @changed="onTrashChanged" />

    <ImportFolderDialog v-if="importOpen" @close="importOpen = false" @imported="onImported" />

    <PolishDialog v-if="polishOpen && polishScope.length" :book-ids="polishScope" @close="polishOpen = false" @done="onPolished" />

    <AnnotationsBrowser v-if="annotationsOpen" @close="annotationsOpen = false" @open-book="openBookFromAnnotation" />

    <MapperDialog v-if="mapperOpen" :book-ids="mapperScope" @close="mapperOpen = false" @applied="onMapperApplied" />

    <ContextMenu v-if="contextMenu" :x="contextMenu.x" :y="contextMenu.y" :entries="contextEntries" @choose="onContextChoose" @close="contextMenu = null" />

    <ContextMenu v-if="toolbarMenu" :x="toolbarMenu.x" :y="toolbarMenu.y" :entries="toolbarMenuEntries" @choose="onToolbarMenuChoose" @close="toolbarMenu = null" />

    <ContextMenu v-if="sortMenu" :x="sortMenu.x" :y="sortMenu.y" :entries="sortMenuEntries" @choose="onSortMenuChoose" @close="sortMenu = null" />

    <ContextMenu v-if="vlMenu" :x="vlMenu.x" :y="vlMenu.y" :entries="vlMenuEntries" @choose="onVlMenuChoose" @close="vlMenu = null" />

    <ContextMenu v-if="savedMenu" :x="savedMenu.x" :y="savedMenu.y" :entries="savedMenuEntries" @choose="onSavedMenuChoose" @close="savedMenu = null" />

    <NoteEditor v-if="noteTarget" :field="noteTarget.field" :item-name="noteTarget.itemName" @close="noteTarget = null" />

    <ManageListsDialog v-if="manageOpen" :virtual-libraries="virtualLibraries" :saved-searches="savedSearches" :query="activeQuery" @close="manageOpen = false" @changed="loadMetadata" @virtual-library-deleted="onVirtualLibraryDeleted" />

    <ColumnPickerDialog v-if="columnPickerOpen" :columns="pickerColumns" :shown="tablePrefs.columns" @close="columnPickerOpen = false" @toggle="toggleColumn" @move="moveColumn" @reorder="onPickerReorder" @reset="resetColumns" />

    <CustomColumnsDialog v-if="columnsOpen" @close="closeColumns" @changed="onColumnsChanged" />

    <CheckLibraryDialog :open="checkLibraryOpen" @close="checkLibraryOpen = false" @changed="runSearch" />

    <DuplicatesDialog
      :open="duplicatesOpen"
      :store="dupStore"
      :merge-note="mergeDone"
      @close="duplicatesOpen = false"
      @merge="openMergeForGroup"
      @compare="openCompareForGroup"
      @details="(id) => (selectedBookId = id)"
      @resolve="resolveOpen = true"
      @changed="onDuplicatesChanged"
    />

    <ResolveDialog v-if="duplicatesOpen && resolveOpen" :store="dupStore" :controller="resolveController" @close="resolveOpen = false" @changed="onDuplicatesChanged" />
    <CompareDuplicates v-if="compareController.state.open" :controller="compareController" @close="compareController.close()" />
    <MirrorDialog v-if="mirrorOpen" :controller="mirrorController" :selected-ids="organizeSelection" @close="mirrorOpen = false" />
    <NamingDialog v-if="namingOpen" :controller="namingController" :sample-ids="namingSampleIds" :selected-count="organizeSelection.length" @close="namingOpen = false" @apply="applyNaming" />
    <OrganizeDialog v-if="organizeOpen" :controller="organizeController" :selected-ids="organizeSelection" @close="organizeOpen = false" @changed="onDuplicatesChanged" />
    <RetitleDialog v-if="retitleOpen" :controller="retitleController" :selected-ids="retitleSelection" @close="retitleOpen = false" @changed="onRetitleChanged" />

    <MergeBooksDialog v-if="mergeOpen" :books="mergeBooksList" :dest="mergeDest" @close="mergeOpen = false" @merged="onMerged" />

    <RenameFilesDialog v-if="renameOpen" :ids="renameScope" @close="renameOpen = false" @renamed="runSearch" />

    <CopyToLibraryDialog v-if="copyOpen" :ids="renameScope" :targets="copyTargets" @close="copyOpen = false" @moved="runSearch" />

    <EmbedMetadataDialog v-if="embedOpen" :ids="renameScope" @close="embedOpen = false" @embedded="runSearch" />

    <SaveToDiskDialog v-if="saveToDiskOpen" :ids="renameScope" @close="saveToDiskOpen = false" />

    <NewsDialog :open="newsOpen" @close="newsOpen = false" @changed="libraryChanged" />

    <NewLibraryDialog v-if="newLibraryOpen" v-model:name="newLibraryName" :busy="creatingLibrary" :error="newLibraryError" @close="newLibraryOpen = false" @submit="createLibrary" />

    <SwitchLibraryDialog v-if="switchOpen" :recent="recentLibraries" :switching="switching" :error="switchError" @close="switchOpen = false" @choose-recent="switchToRecent" @browse="switchToOther" />
  </div>
</template>

<style scoped>
.library {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
/* Two purposeful rows rather than one wrapping flex: a wrapping
   toolbar reflows as buttons come and go with the selection, so the
   list jumps down the screen while you are reading it. Fixed rows keep
   the content below at a constant height. */
.toolbar {
  display: flex;
  flex-direction: column;
  gap: 0.35em;
  padding: 0.4em 0.5em;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.toolbar-row {
  display: flex;
  align-items: center;
  gap: 0.4em;
  /* One line each. Anything that does not fit scrolls rather than
     wrapping onto a new row. */
  flex-wrap: nowrap;
  overflow-x: auto;
  overflow-y: hidden;
}
.toolbar-row > * {
  flex-shrink: 0;
}
/* Pushes Settings to the far end, the way a toolbar's least-used
   control usually sits. */
.toolbar-gap {
  flex: 1 1 auto;
  min-width: 0;
}
/* A 1px rule at 60% height, not a full-height divider: a toolbar
   separator should group, not fence. */
.tb-sep {
  flex: 0 0 1px;
  align-self: center;
  height: 60%;
  background: var(--border);
  margin: 0 var(--sp-2);
}
/* The action row is taller than the find row and sets its own height
   from the button, so it must not be squeezed by the flex column. */
.toolbar-actions {
  min-height: var(--tb-h);
  gap: var(--sp-1);
}
.toolbar-find {
  min-height: 34px;
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
  background: var(--accent-soft);
  border-radius: var(--radius-md);
  padding: 0.2em 0.4em;
  font-size: var(--fs-small);
}
.sort-chip-remove {
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  font-size: var(--fs-body);
  line-height: 1;
}
/*
  The field, its mode toggle, its clear button and its submit read as
  one control: the border is on the form and the children are
  transparent, so the icons sit *inside* the field rather than beside
  it. Separate bordered buttons next to a bordered input is the web-form
  look this is meant to get away from.
*/
.search {
  display: flex;
  align-items: center;
  flex: 1;
  min-width: 220px;
  max-width: 560px;
  height: var(--ctl-h);
  padding: 0 var(--sp-1);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
}
.search:focus-within {
  border-color: var(--accent);
  box-shadow: var(--focus-ring);
}
.search input {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: none;
  background: none;
  padding: 0 var(--sp-2);
}
.search input:focus-visible {
  outline: none;
  box-shadow: none;
}
/* Strip the browser's own search affordances -- it draws a second
   clear button that does not match ours. */
.search input::-webkit-search-cancel-button {
  display: none;
}
.in-field,
.search-go {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 20px;
  min-width: 20px;
  padding: 0 var(--sp-1);
  border: none;
  background: none;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: var(--fs-small);
}
.in-field:hover,
.search-go:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.in-field.active {
  background: var(--accent-soft);
  color: var(--accent);
}
.in-field-icon {
  color: currentColor;
}

/* The active virtual library is named next to its button, because
   which subset of the library you are looking at is the kind of state
   that should never need a click to discover. */
.vl-name {
  font-size: var(--fs-small);
  color: var(--fg-faint);
  white-space: nowrap;
  max-width: 18ch;
  overflow: hidden;
  text-overflow: ellipsis;
}
.vl-name.active {
  color: var(--accent);
  font-weight: 600;
}
.vl-clear {
  height: var(--ctl-h-sm);
  min-width: var(--ctl-h-sm);
  padding: 0;
  font-size: var(--fs-small);
}
.body {
  flex: 1;
  display: flex;
  overflow: hidden;
}
.sidebar {
  /* Width is set inline from the saved layout and the drag. */
  flex-shrink: 0;
  overflow: auto;
  min-height: 0;
}
.details-column {
  flex-shrink: 0;
  min-height: 0;
  overflow: hidden;
  border-left: 1px solid var(--border);
}
/* A thin, full-height grab strip between panels. It is deliberately
   wider on hover than at rest: 4px is the right visual weight for a
   divider but an awkward target, so the hit area grows once the
   pointer is near it. */
.splitter {
  flex: 0 0 4px;
  cursor: col-resize;
  background: var(--border);
  transition: background 0.12s ease;
  touch-action: none;
}
.splitter:hover,
.splitter:active {
  background: var(--accent);
}
.status-bar {
  display: flex;
  align-items: center;
  gap: 0.75em;
  padding: 0.3em 0.75em;
  border-top: 1px solid var(--border);
  font-size: var(--fs-small);
  flex-shrink: 0;
  background: var(--bg-bar);
}
.status-spacer {
  flex: 1;
}
.status-count {
  font-weight: 600;
}
/* The library label reads as a control because it is one -- clicking
   it switches library, which is where anyone looking at the name is
   most likely headed next. */
.storage-banner {
  display: flex;
  align-items: flex-start;
  gap: 0.75em;
  padding: 0.4em 0.75em;
  font-size: var(--fs-body);
  background: color-mix(in srgb, var(--accent) 14%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
}
.storage-banner.warning {
  background: color-mix(in srgb, var(--warning) 20%, transparent);
  border-bottom-color: color-mix(in srgb, var(--warning) 55%, transparent);
}
.storage-banner-text {
  flex: 1;
}
.storage-banner-close {
  border: none;
  background: transparent;
  color: inherit;
  font-size: 1.2em;
  line-height: 1;
  cursor: pointer;
}
.status-library {
  padding: 0.1em 0.5em;
  font-size: var(--fs-body);
  max-width: 28ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status-sel {
  opacity: 0.75;
}
.status-draft {
  opacity: 1;
  color: var(--warning);
  font-weight: 600;
}
.empty-state {
  margin: var(--sp-8) auto;
  max-width: 420px;
  padding: 0 var(--sp-6);
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--sp-4);
}
.empty-state h3 {
  margin: 0;
  font-size: var(--fs-medium);
}
.empty-state .hint {
  margin: 0;
  color: var(--fg-muted);
}
.empty-actions {
  display: flex;
  gap: var(--sp-4);
  justify-content: center;
}
.filter-chips {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-wrap: wrap;
  gap: var(--sp-3);
  justify-content: center;
}
.filter-chip {
  display: inline-flex;
  align-items: center;
  gap: var(--sp-2);
  padding: 0 var(--sp-3) 0 var(--sp-4);
  height: var(--ctl-h);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-pill);
  background: var(--bg-sunken);
}
.filter-chip button {
  border: none;
  background: none;
  height: auto;
  padding: 0 var(--sp-2);
  color: var(--fg-muted);
}
.status-sort {
  opacity: 0.7;
}
.status-toggle {
  padding: 0.15em 0.6em;
  font-size: var(--fs-body);
}
.grid-area {
  flex: 1;
  /* `min-width: 0` is load-bearing: without it a flex item refuses to
     shrink below its content, so a wide book table pushes the details
     column out of the window rather than scrolling within its own. */
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  /* `hidden`, not `auto`: the list scrolls inside itself so the
     status bar below it stays put instead of scrolling away. */
  overflow: hidden;
  padding: 0.5em;
}
.status,
.error {
  padding: 1em;
}
.error {
  color: var(--danger);
}
.add-error,
.add-summary {
  padding: 0 0.5em;
}
.hidden-file-input {
  display: none;
}
.toolbar button.active {
  background: var(--accent-solid);
  color: var(--fg-on-accent);
  border-color: var(--accent-solid);
}
/* Reachable by keyboard only: the first Tab stop, jumping past the toolbar and the category tree. */
.skip-link {
  position: absolute;
  left: -9999px;
  top: 0;
  z-index: var(--z-popover);
  padding: var(--sp-3) var(--sp-5);
  background: var(--bg-raised);
  color: var(--fg);
  border: 1px solid var(--border-strong);
}
.skip-link:focus {
  left: var(--sp-4);
  top: var(--sp-4);
}
.action-notice {
  margin: 0;
  padding: var(--sp-2) var(--sp-4);
  background: var(--accent-soft);
  border-bottom: 1px solid var(--border);
}
/* In a narrow window the side panels do not share the row with the list: each opens as a
   drawer over it, from the status bar's Categories / Details buttons. */
.body {
  position: relative;
}
.sidebar-drawer,
.details-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  z-index: var(--z-popover);
  background: var(--bg);
  box-shadow: var(--shadow-menu);
  max-width: 92vw;
}
.sidebar-drawer {
  left: 0;
  max-width: 85vw;
}
.details-drawer {
  right: 0;
  width: min(400px, 92vw);
}
.bulk-dock {
  overflow: auto;
  background: var(--bg-sunken);
}
.settings-link {
  padding: 0.35em 0.7em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
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
  color: var(--warning);
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
  border-bottom: 1px solid var(--border);
  cursor: pointer;
}
.fts-result:hover {
  background: var(--bg-sunken);
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
  color: var(--fg-muted);
  font-size: var(--fs-body);
}
.fts-result-formats {
  color: var(--fg-faint);
  font-size: var(--fs-small);
  text-transform: uppercase;
  margin-left: auto;
}
.fts-snippet {
  margin: 0.4em 0 0;
  font-size: var(--fs-body);
  color: var(--fg-muted);
}
.fts-snippet mark {
  background: var(--highlight);
  color: inherit;
  padding: 0 0.1em;
}
.dup-filter {
  display: inline-flex;
  gap: 0.4rem;
  align-items: center;
  margin: 0.5rem 0;
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
/* Marks (#1.5). */
.marks-indicator {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.15rem 0.45rem;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
}
.marks-count {
  font-size: var(--fs-small);
  opacity: 0.8;
  white-space: nowrap;
}

</style>
