<script setup lang="ts">
import { useKeepScroll } from "../library/keepScroll";
import Dialog from "./Dialog.vue";
import { confirmDialog } from "./dialogs/confirm";
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import CollapsibleSection from "./CollapsibleSection.vue";
import ImageViewer from "./ImageViewer.vue";
import { loadSections, saveSections, toggleSection, type SectionId, type SectionState } from "../library/panelSections";
import AutocompleteInput from "./AutocompleteInput.vue";
import { completionForCustom, enumValuesOf, invalidateForChanges } from "../library/fieldValues";
import { changesFromForm, cloneForm, formFromBook, isFormDirty, rebaseForm, sharedDrafts, type MetaForm } from "../library/metadataForm";
import { loadPdfJs, PDF_ASSET_OPTIONS, pdfUrl, renderPage, renderPageToBlob, scaleForWidth } from "../reader/pdf";
import { useRouter } from "vue-router";
import FetchMetadataDialog from "./FetchMetadataDialog.vue";
import TweakEditor from "./TweakEditor.vue";
import { trashBooksFlow } from "../library/deleteFlow";
import { addFormat, coverFromPdfPage, evaluateTemplate, fetchBook, fetchBooks, fetchConversionBookData, fetchDataFiles, fetchFieldMetadata, getConversionStatus, removeDataFile, removeFormat, search, setCover, setFields, shareEmail, startConversion, uploadDataFile } from "../library/api";
import type { ConversionOptionsOverride, DataFileStat, SmtpRelayConfig } from "../library/api";
import { categoryItemToQuery } from "../library/query";
import { isTauri, tauriInvoke } from "../tauri";
import { DEFAULT_SAVED_TEMPLATES, removeTemplate, saveTemplate, SAVED_TEMPLATES_PROFILE, TemplateNameError, wouldOverwrite, type SavedTemplate, type SavedTemplates } from "../library/savedTemplates";
import { fetchProfile, saveProfile } from "../settings/api";
import type { BookSummary, FieldMetaEntry } from "../library/types";
import { getItemCompat } from "../library/legacyStorage";

const props = defineProps<{
  bookId: number;
  /**
   * Render as a docked column rather than a centred modal.
   *
   * calibre keeps book details permanently visible beside the list,
   * and reading a row's details there costs no interaction. As a modal
   * it covers the list it describes and has to be dismissed before the
   * next row can be looked at, which is the opposite of how the panel
   * is actually used.
   */
  docked?: boolean;
  /**
   * An action to perform as soon as the book has loaded -- how the
   * right-click menu (#1.2) reaches the panel's own controls. A
   * counter accompanies it so choosing the *same* action twice in a
   * row still fires: watching the id alone would see no change.
   */
  pendingAction?: { id: string; nonce: number } | null;
  /** Bumped when the book was changed elsewhere (an in-place edit in the list). */
  refreshKey?: number;
}>();
const emit = defineEmits<{ close: []; updated: []; deleted: [bookId: number]; "open-book": [bookId: number]; "step-book": [delta: number] }>();
const router = useRouter();

const book = ref<BookSummary | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);

let deletedId: number | null = null;
const deleting = ref(false);
const deleteError = ref<string | null>(null);

async function deleteBook() {
  // The book that is on screen, not whatever props.bookId says by the time
  // the answer comes back: a stale panel must never delete a different book.
  const b = book.value;
  if (!b) return;
  const id = b.id;
  deleting.value = true;
  deleteError.value = null;
  try {
    const result = await trashBooksFlow([{ id, title: b.title }], { onRestored: () => emit("updated") });
    if (result.cancelled) return;
    if (result.failureText) deleteError.value = result.failureText;
    if (result.outcome?.deleted.includes(id)) {
      drafts.discard(id); // an edit to a book that is gone has nowhere to go
      deletedId = id;
      emit("deleted", id);
    }
  } catch (e) {
    deleteError.value = e instanceof Error ? e.message : String(e);
  } finally {
    deleting.value = false;
  }
}

const convertOpen = ref(false);
const convertLoadingFormats = ref(false);
const convertInputFormats = ref<string[]>([]);
const convertOutputFormats = ref<string[]>([]);
const convertInputFmt = ref("");
const convertOutputFmt = ref("");
const converting = ref(false);
const convertError = ref<string | null>(null);
const convertDone = ref(false);

// A real, deliberate first-slice subset of upstream's ~40 conversion
// options -- see crates/lexmechanic_srv/src/convert.rs's own doc for why.
const convertUnsmartenPunctuation = ref(false);
const convertLinearizeTables = ref(false);
const convertInsertMetadata = ref(false);
const convertRemoveFirstImage = ref(false);
const convertUseAutoToc = ref(false);
const convertChapter = ref("");
const convertMaxTocLinks = ref("");
const convertBaseFontSize = ref("");

async function openConvert() {
  convertOpen.value = true;
  convertDone.value = false;
  convertError.value = null;
  convertLoadingFormats.value = true;
  convertUnsmartenPunctuation.value = false;
  convertLinearizeTables.value = false;
  convertInsertMetadata.value = false;
  convertRemoveFirstImage.value = false;
  convertUseAutoToc.value = false;
  convertChapter.value = "";
  convertMaxTocLinks.value = "";
  convertBaseFontSize.value = "";
  try {
    const data = await fetchConversionBookData(props.bookId);
    convertInputFormats.value = data.input_formats;
    convertOutputFormats.value = data.output_formats;
    convertInputFmt.value = data.input_formats[0] ?? "";
    // Default to a real format the book doesn't already have, so
    // starting a conversion produces a genuinely new format rather
    // than one already sitting in the format list.
    convertOutputFmt.value = data.output_formats.find((f) => !data.input_formats.includes(f)) ?? data.output_formats[0] ?? "";
  } catch (e) {
    convertError.value = e instanceof Error ? e.message : String(e);
  } finally {
    convertLoadingFormats.value = false;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function runConversion() {
  if (!convertInputFmt.value || !convertOutputFmt.value) return;
  converting.value = true;
  convertError.value = null;
  convertDone.value = false;
  try {
    const options: ConversionOptionsOverride = {};
    if (convertUnsmartenPunctuation.value) options.unsmarten_punctuation = true;
    if (convertLinearizeTables.value) options.linearize_tables = true;
    if (convertInsertMetadata.value) options.insert_metadata = true;
    if (convertRemoveFirstImage.value) options.remove_first_image = true;
    if (convertUseAutoToc.value) options.use_auto_toc = true;
    if (convertChapter.value.trim()) options.chapter = convertChapter.value.trim();
    if (convertMaxTocLinks.value.trim()) options.max_toc_links = Number(convertMaxTocLinks.value);
    if (convertBaseFontSize.value.trim()) options.base_font_size = Number(convertBaseFontSize.value);
    const jobId = await startConversion(props.bookId, convertInputFmt.value, convertOutputFmt.value, options);
    for (;;) {
      const status = await getConversionStatus(jobId);
      if (!status.running) {
        if (!status.ok) {
          throw new Error(status.traceback || "conversion failed");
        }
        break;
      }
      await sleep(700);
    }
    convertDone.value = true;
    book.value = await fetchBook(props.bookId);
    emit("updated");
  } catch (e) {
    convertError.value = e instanceof Error ? e.message : String(e);
  } finally {
    converting.value = false;
  }
}

// Send via email -- real POST /share/email. No persisted SMTP account
// server-side yet (crates/lexmechanic_srv/src/share.rs's own doc: ties
// into the not-yet-built preferences epic), so the relay config is
// remembered client-side in localStorage instead, purely for this
// browser's own convenience across sends.
const RELAY_STORAGE_KEY = "lexmechanic-smtp-relay";

function loadSavedRelay(): SmtpRelayConfig {
  try {
    const raw = getItemCompat(localStorage, RELAY_STORAGE_KEY);
    if (raw) return JSON.parse(raw) as SmtpRelayConfig;
  } catch {
    // Ignore a corrupt/unavailable localStorage entry -- fall through
    // to real, empty defaults below rather than failing to open the
    // share panel at all.
  }
  return { relay: "", encryption: "tls" };
}

const shareOpen = ref(false);
const shareFormat = ref("");
const shareFrom = ref("");
const shareTo = ref("");
const shareSubject = ref("");
const shareRelay = ref<SmtpRelayConfig>(loadSavedRelay());
const sharing = ref(false);
const shareError = ref<string | null>(null);
const shareDone = ref(false);

function openShare() {
  shareOpen.value = true;
  shareDone.value = false;
  shareError.value = null;
  shareFormat.value = book.value?.formats[0] ?? "";
  shareSubject.value = book.value?.title ?? "";
}

async function sendShareEmail() {
  if (!shareFormat.value || !shareFrom.value || !shareTo.value || !shareRelay.value.relay) return;
  sharing.value = true;
  shareError.value = null;
  shareDone.value = false;
  try {
    await shareEmail(props.bookId, shareFormat.value, shareFrom.value, shareTo.value, shareRelay.value, shareSubject.value || undefined);
    localStorage.setItem(RELAY_STORAGE_KEY, JSON.stringify(shareRelay.value));
    shareDone.value = true;
  } catch (e) {
    shareError.value = e instanceof Error ? e.message : String(e);
  } finally {
    sharing.value = false;
  }
}

// ---------------------------------------------------------------
// Sections
// ---------------------------------------------------------------
//
// The panel is a stack of collapsible sections rather than separate
// views, so the preview and the metadata form can be open together.
// Which are open is remembered per section (library/panelSections.ts).

const sections = ref<SectionState>(loadSections());

function toggle(id: SectionId) {
  sections.value = toggleSection(sections.value, id);
  saveSections(sections.value);
}

function openSection(id: SectionId) {
  if (sections.value[id]) return;
  sections.value = { ...sections.value, [id]: true };
  saveSections(sections.value);
}

// ---------------------------------------------------------------
// Metadata: always-available inline form
// ---------------------------------------------------------------
//
// Unsaved edits when another book is selected: a draft is kept *per
// book* (see DraftStore) and restored on return. Prompting instead
// would interrupt every arrow-key step down the list.

const saving = ref(false);
const saveError = ref<string | null>(null);
const form = ref<MetaForm | null>(null);
const baseline = ref<MetaForm | null>(null);
// Module-level, so a draft survives this panel unmounting (collapsed, reader opened...).
const drafts = sharedDrafts;
const coverInput = ref<HTMLInputElement | null>(null);

const dirty = computed(() => (form.value && baseline.value ? isFormDirty(form.value, baseline.value) : false));

// Custom columns (issue #720) -- discovered from /ajax/field-metadata
// (real, already includes custom columns, see that route's own doc),
// rendered/edited generically alongside the fixed fields. `key` is
// "#label"; `label` is the bare name the book row and set-fields'
// `changes` object actually use.
const customColumnFields = ref<FieldMetaEntry[]>([]);

async function loadCustomColumnFields() {
  try {
    const fm = await fetchFieldMetadata();
    customColumnFields.value = Object.values(fm.field_metadata).filter((f) => f.is_custom);
  } catch (e) {
    console.error("failed to load custom column metadata", e);
  }
}
void loadCustomColumnFields();

/** (Re)fills the form from a book, restoring this book's draft if it has one. */
function fillForm(b: BookSummary) {
  const fresh = formFromBook(b, customColumnFields.value);
  baseline.value = fresh;
  const draft = drafts.restore(b.id, fresh);
  form.value = draft ?? cloneForm(fresh);
}

// The shared store follows the form as it is edited, so the library
// view's "N unsaved drafts" count is live and nothing depends on a
// teardown hook running.
watch(
  [form, baseline],
  () => {
    if (!form.value || !baseline.value || !book.value || book.value.id !== props.bookId || deletedId === props.bookId) return;
    drafts.stash(props.bookId, form.value, baseline.value);
  },
  { deep: true },
);

// Collapsing the panel, opening the reader or deleting the book unmounts
// it: the unsaved edits go to the shared store, not nowhere.
onBeforeUnmount(() => {
  if (form.value && baseline.value && book.value && book.value.id === props.bookId && deletedId !== props.bookId) drafts.stash(props.bookId, form.value, baseline.value);
});

// Custom column definitions can arrive after the book.
watch(customColumnFields, () => {
  if (book.value && !dirty.value) fillForm(book.value);
});

function revert() {
  if (!baseline.value) return;
  form.value = cloneForm(baseline.value);
  drafts.discard(props.bookId);
  saveError.value = null;
}

/** Opens the Metadata section and focuses the title (the "Edit metadata" menu item). */
function startEditing() {
  openSection("metadata");
  void nextTick(() => rootEl.value?.querySelector<HTMLInputElement>("[data-field='title']")?.focus());
}

const rootEl = ref<HTMLElement | null>(null);
useKeepScroll(rootEl);

async function saveEdits() {
  if (!form.value || !dirty.value) return;
  // The book can change while the request is in flight (arrow-key to the
  // next row): everything after the await is judged against this id.
  const id = props.bookId;
  saving.value = true;
  saveError.value = null;
  try {
    // Only the fields edited here are sent, so a later Save cannot revert
    // a change made elsewhere (a table cell).
    const changes = changesFromForm(form.value, customColumnFields.value, baseline.value);
    const updated = await setFields(id, changes);
    invalidateForChanges(changes);
    drafts.discard(id);
    emit("updated");
    if (id !== props.bookId) return; // moved on: do not paint this book's data over the new one
    book.value = updated;
    fillForm(updated);
  } catch (e) {
    if (id === props.bookId) saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

async function replaceCover(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  const id = props.bookId;
  saving.value = true;
  saveError.value = null;
  try {
    await setCover(id, file);
    const fresh = await fetchBook(id);
    emit("updated");
    if (id === props.bookId) book.value = fresh;
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
    if (coverInput.value) coverInput.value.value = "";
  }
}

/**
 * Regenerates the cover from the PDF's first page.
 *
 * Offered only when there is a PDF to render: on any other book the
 * action would fail with a 404 that says nothing useful, and a
 * disabled entry explains itself.
 */
const canCoverFromPdf = computed(() => (book.value?.formats ?? []).some((f) => f.toLowerCase() === "pdf"));

async function coverFromPdf() {
  if (!canCoverFromPdf.value) {
    // Reachable from the context menu, which enables book actions by
    // how many books are selected and knows nothing about formats.
    // Saying so beats doing nothing.
    saveError.value = "This book has no PDF to render a cover from.";
    return;
  }
  saving.value = true;
  saveError.value = null;
  try {
    await coverFromPdfPage(props.bookId, 1);
    book.value = await fetchBook(props.bookId);
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

const formatInput = ref<HTMLInputElement | null>(null);
const formatBusy = ref(false);

async function addFormatFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  formatBusy.value = true;
  saveError.value = null;
  try {
    book.value = await addFormat(props.bookId, file);
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    formatBusy.value = false;
    if (formatInput.value) formatInput.value.value = "";
  }
}

async function removeFormatClick(ext: string) {
  if (!(await confirmDialog(`Remove the ${ext.toUpperCase()} format from this book?`, { confirmLabel: "Remove", danger: true }))) return;
  formatBusy.value = true;
  saveError.value = null;
  try {
    book.value = await removeFormat(props.bookId, ext);
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    formatBusy.value = false;
  }
}

// Data files (issue #757) -- arbitrary files attached to a book
// outside its standard formats. Loaded on demand (only once the edit
// form is open) rather than alongside the book's own metadata, since
// most books never have any and it's a real, separate fetch.
const dataFiles = ref<Record<string, DataFileStat>>({});
const dataFilesLoading = ref(false);
const dataFileInput = ref<HTMLInputElement | null>(null);
const dataFileBusy = ref(false);

// `data/notes.pdf` -> `notes.pdf` for display -- the `data/` prefix
// is this feature's own internal storage convention
// (`extra_files::add_extra_files`'s own `format!("data/{name}")`),
// not something a user should have to see or type.
function dataFileName(relpath: string): string {
  return relpath.startsWith("data/") ? relpath.slice("data/".length) : relpath;
}

async function loadDataFiles() {
  dataFilesLoading.value = true;
  try {
    dataFiles.value = await fetchDataFiles(props.bookId);
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    dataFilesLoading.value = false;
  }
}

async function addDataFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  dataFileBusy.value = true;
  saveError.value = null;
  try {
    dataFiles.value = await uploadDataFile(props.bookId, file);
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    dataFileBusy.value = false;
    if (dataFileInput.value) dataFileInput.value.value = "";
  }
}

async function removeDataFileClick(relpath: string) {
  if (!(await confirmDialog(`Remove "${dataFileName(relpath)}" from this book?`, { confirmLabel: "Remove", danger: true }))) return;
  dataFileBusy.value = true;
  saveError.value = null;
  try {
    dataFiles.value = await removeDataFile(props.bookId, relpath);
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    dataFileBusy.value = false;
  }
}

// Quick View (issue #758) -- confirmed while scoping the issue that
// this needs no new backend at all: categoryItemToQuery + the
// already-real /ajax/search route already produce exactly "every
// other book sharing this author/tag/series" -- this is purely a
// frontend composition over existing pieces.
interface QuickViewGroup {
  label: string;
  books: BookSummary[];
}

const quickViewOpen = ref(false);
const quickViewLoading = ref(false);
const quickViewGroups = ref<QuickViewGroup[]>([]);

async function loadQuickView() {
  const b = book.value;
  if (!b) return;
  quickViewLoading.value = true;
  saveError.value = null;
  try {
    const candidates: { category: string; label: string }[] = [
      ...(b.authors ?? []).map((label) => ({ category: "authors", label })),
      ...(b.tags ?? []).map((label) => ({ category: "tags", label })),
      ...(b.series ? [{ category: "series", label: b.series }] : []),
    ];
    const groups: QuickViewGroup[] = [];
    for (const { category, label } of candidates) {
      const result = await search({ query: categoryItemToQuery(category, label), num: 6, offset: 0, sort: "title", sortOrder: "asc", vl: "" });
      const otherIds = result.book_ids.filter((id) => id !== props.bookId);
      if (otherIds.length === 0) continue;
      groups.push({ label, books: await fetchBooks(otherIds) });
    }
    quickViewGroups.value = groups;
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    quickViewLoading.value = false;
  }
}

function openQuickView() {
  quickViewOpen.value = true;
  if (quickViewGroups.value.length === 0) void loadQuickView();
}

function openBookFromQuickView(id: number) {
  quickViewOpen.value = false;
  emit("open-book", id);
}

// Template tester (issue #763) -- evaluated against the currently-open
// book, matching this component's own established modal-panel pattern
// (Quick View, Send…, etc.) rather than a separate book-picker UI.
const templateTesterOpen = ref(false);
const templateInput = ref("field('title')");
const templateRunning = ref(false);
const templateResult = ref<string | null>(null);
const templateError = ref<string | null>(null);

// Saved templates (#1.15). The tester could evaluate a template but
// not keep one, so every useful expression had to be retyped. The
// mutation rules live in library/savedTemplates.ts -- the interesting
// cases are all name collisions, which silently overwrite work if
// handled wrong.
const savedTemplates = ref<SavedTemplate[]>([]);
const templateNameInput = ref("");
const templateSaveError = ref<string | null>(null);

async function loadSavedTemplates() {
  try {
    const stored = await fetchProfile<SavedTemplates>(SAVED_TEMPLATES_PROFILE);
    savedTemplates.value = stored?.templates ?? DEFAULT_SAVED_TEMPLATES.templates;
  } catch (e) {
    console.error("failed to load saved templates", e);
  }
}
void loadSavedTemplates();

async function persistTemplates(next: SavedTemplate[]) {
  savedTemplates.value = next;
  try {
    await saveProfile(SAVED_TEMPLATES_PROFILE, { templates: next });
  } catch (e) {
    templateSaveError.value = e instanceof Error ? e.message : String(e);
  }
}

async function saveCurrentTemplate() {
  templateSaveError.value = null;
  const name = templateNameInput.value;
  // Say so *before* overwriting, not after.
  if (wouldOverwrite(savedTemplates.value, name) && !(await confirmDialog(`Replace the saved template "${name.trim()}"?`, { confirmLabel: "Replace" }))) return;
  try {
    await persistTemplates(saveTemplate(savedTemplates.value, name, templateInput.value));
    templateNameInput.value = "";
  } catch (e) {
    templateSaveError.value = e instanceof TemplateNameError ? e.message : String(e);
  }
}

function loadSavedTemplate(name: string) {
  const found = savedTemplates.value.find((t) => t.name === name);
  if (found) templateInput.value = found.template;
}

async function deleteSavedTemplate(name: string) {
  await persistTemplates(removeTemplate(savedTemplates.value, name));
}

async function runTemplateTest() {
  templateRunning.value = true;
  templateResult.value = null;
  templateError.value = null;
  try {
    const r = await evaluateTemplate(props.bookId, templateInput.value);
    if (r.ok) templateResult.value = r.result ?? "";
    else templateError.value = r.error ?? "Unknown template error";
  } catch (e) {
    templateError.value = e instanceof Error ? e.message : String(e);
  } finally {
    templateRunning.value = false;
  }
}

// Formats the in-app reader can show. EPUB/KEPUB go through
// `render_book`; PDF is rendered by PDF.js (#816 item 2.1), which
// needs no server-side rendering at all -- it reads `/get/pdf/{id}`
// directly.
//
// EPUB first: when a book has both, the EPUB reader offers contents,
// bookmarks, highlights and read-aloud that the PDF view cannot.
const READABLE_FORMATS = ["epub", "kepub", "pdf"];

const readableFormat = computed(() => book.value?.formats.find((f) => READABLE_FORMATS.includes(f)) ?? null);

// "Open externally" (#818). The in-app reader handles EPUB/KEPUB only
// (`is_viewable_format` in lexmechanic_srv), so for a PDF-first library
// this is the only way to actually open a book -- and it stays useful
// for any format the reader will never render.
//
// Desktop-only: handing a file to the OS default application is
// exactly what a browser tab cannot do.
const openingExternally = ref(false);
const openExternallyError = ref<string | null>(null);

/// Prefers a format the in-app reader *cannot* show: if a book has
/// both an EPUB and a PDF, "Read" already covers the EPUB, so the
/// useful thing to hand to the system viewer is the PDF.
const externalFormat = computed(() => {
  const formats = book.value?.formats ?? [];
  return formats.find((f) => !READABLE_FORMATS.includes(f)) ?? formats[0] ?? null;
});

// Unpack / repack (#1.12). Hands the book to whatever editor the
// user prefers, then rebuilds it. `implode` verifies a marker file
// `explode` left behind, so a folder that was never unpacked -- or
// was unpacked from a different format -- is refused rather than
// producing a broken book.
const unpacking = ref(false);
const unpackMessage = ref<string | null>(null);

/** Only formats `lexmechanic_ebooks::tweak::get_tools` knows how to explode. */
const UNPACKABLE_FORMATS = ["epub", "azw3", "mobi", "azw", "docx", "htmlz"];
const unpackableFormat = computed(() => book.value?.formats.find((f) => UNPACKABLE_FORMATS.includes(f)) ?? null);
const canUnpack = computed(() => isTauri() && unpackableFormat.value !== null);

async function unpackBook() {
  const fmt = unpackableFormat.value;
  if (!fmt) return;
  unpacking.value = true;
  unpackMessage.value = null;
  openExternallyError.value = null;
  try {
    const result = await tauriInvoke<{ path: string } | null>("unpack_book", { bookId: props.bookId, fmt });
    if (result) unpackMessage.value = `Unpacked to ${result.path}`;
  } catch (e) {
    openExternallyError.value = e instanceof Error ? e.message : String(e);
  } finally {
    unpacking.value = false;
  }
}

async function repackBook() {
  const fmt = unpackableFormat.value;
  if (!fmt) return;
  unpacking.value = true;
  unpackMessage.value = null;
  openExternallyError.value = null;
  try {
    const done = await tauriInvoke<boolean>("repack_book", { bookId: props.bookId, fmt });
    if (done) {
      unpackMessage.value = "Rebuilt and saved back to the library.";
      emit("updated");
    }
  } catch (e) {
    openExternallyError.value = e instanceof Error ? e.message : String(e);
  } finally {
    unpacking.value = false;
  }
}

/** Shows this book's files in the system file browser (#886). */
async function revealBookFolder() {
  // With a format to point at, select that file; a book with no files
  // (or an older desktop build) falls back to opening the folder.
  const fmt = book.value?.formats[0];
  if (fmt) {
    await revealFile(fmt);
    return;
  }
  openExternallyError.value = null;
  try {
    await tauriInvoke<void>("reveal_book_folder", { bookId: props.bookId });
  } catch (e) {
    openExternallyError.value = e instanceof Error ? e.message : String(e);
  }
}

/** Opens the system file browser with this format's file selected (desktop only). */
async function revealFile(fmt: string) {
  openExternallyError.value = null;
  try {
    await tauriInvoke<void>("reveal_book_file", { bookId: props.bookId, fmt });
  } catch (e) {
    openExternallyError.value = e instanceof Error ? e.message : String(e);
  }
}

/** In a plain browser there is no file manager to open, so the path is offered for copying. */
const desktop = isTauri();
const copied = ref(false);

async function copyFolderPath() {
  const path = (book.value?.path as string | undefined) ?? "";
  try {
    await navigator.clipboard.writeText(path);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    // clipboard unavailable: the path is selectable text anyway
  }
}

/** Format sizes, read from the Content-Length of a HEAD request. */
const formatSizes = ref<Record<string, number>>({});

async function loadFormatSizes() {
  const id = props.bookId;
  const links = formatLinks.value;
  const sizes: Record<string, number> = {};
  await Promise.all(
    links.map(async ([fmt, url]) => {
      try {
        const r = await fetch(url, { method: "HEAD" });
        const n = Number(r.headers.get("content-length"));
        if (r.ok && Number.isFinite(n) && n > 0) sizes[fmt] = n;
      } catch {
        // no size shown
      }
    }),
  );
  if (id === props.bookId) formatSizes.value = sizes;
}

function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function openFormat(fmt: string) {
  void openExternally(fmt);
}

async function openExternally(chosen?: string) {
  const fmt = chosen ?? externalFormat.value;
  if (!fmt) return;
  openingExternally.value = true;
  openExternallyError.value = null;
  try {
    await tauriInvoke<void>("open_book_format", { bookId: props.bookId, fmt });
  } catch (e) {
    openExternallyError.value = e instanceof Error ? e.message : String(e);
  } finally {
    openingExternally.value = false;
  }
}

// Tweak Book (issue #719) -- crates/lexmechanic_srv/src/tweak.rs's own
// scope is real-EPUB-container-only for this first slice, so the
// action is only offered when the book actually has that format.
const canTweak = computed(() => book.value?.formats.includes("epub") ?? false);
const tweakOpen = ref(false);

async function onTweakUpdated() {
  book.value = await fetchBook(props.bookId);
  emit("updated");
}

const fetchMetadataOpen = ref(false);

async function onFetchMetadataUpdated() {
  book.value = await fetchBook(props.bookId);
  emit("updated");
}

const formatLinks = computed<[string, string][]>(() => {
  const b = book.value;
  if (!b) return [];
  const links: [string, string][] = [];
  if (b.main_format) links.push(...(Object.entries(b.main_format) as [string, string][]));
  links.push(...(Object.entries(b.other_formats) as [string, string][]));
  return links;
});

async function load(id: number) {
  loading.value = true;
  error.value = null;
  book.value = null;
  form.value = null;
  baseline.value = null;
  formatSizes.value = {};
  dataFiles.value = {};
  convertOpen.value = false;
  shareOpen.value = false;
  quickViewOpen.value = false;
  quickViewGroups.value = [];
  templateTesterOpen.value = false;
  templateResult.value = null;
  templateError.value = null;
  try {
    const fetched = await fetchBook(id);
    if (id !== props.bookId) return; // the selection moved on while this was loading
    book.value = fetched;
    fillForm(fetched);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (id === props.bookId) loading.value = false;
  }
}
watch(
  () => props.bookId,
  (id, previous) => {
    // Keep unsaved edits as a draft for the book being left.
    if (previous !== undefined && form.value && baseline.value) drafts.stash(previous, form.value, baseline.value);
    return load(id);
  },
  { immediate: true },
);
// The list edited this book in place: re-read it. A clean form follows
// the new values; a dirty one keeps what the user typed.
watch(
  () => props.refreshKey,
  async () => {
    const id = props.bookId;
    try {
      const fresh = await fetchBook(id);
      if (id === props.bookId) book.value = fresh;
    } catch {
      // The next full load will report it.
    }
  },
);

// The cover/thumbnail URLs are stable paths (`/get/cover/{id}`) --
// after replacing the cover the browser would otherwise keep showing
// the cached old image. Bust the cache with a query param that
// changes only when the book data itself was just reloaded.
const coverVersion = ref(0);
watch(book, (b) => {
  coverVersion.value++;
  // The book was re-read (an in-place edit in the list, a conversion, a
  // fetched cover...). A clean form follows the new values; a dirty one
  // keeps what the user typed and only moves its baseline.
  if (!b || b.id !== props.bookId || !baseline.value) return;
  if (dirty.value && form.value) {
    // Fields the user did not touch follow the new values; edited ones keep what was typed.
    const fresh = formFromBook(b, customColumnFields.value);
    form.value = rebaseForm(form.value, baseline.value, fresh);
    baseline.value = fresh;
  } else fillForm(b);
});
const coverSrc = computed(() => (book.value ? `${book.value.cover}?v=${coverVersion.value}` : ""));

// ---------------------------------------------------------------
// Preview (PDF pages via pdf.js) and the full-window viewer
// ---------------------------------------------------------------
//
// Loaded lazily: nothing is fetched or rendered until the Preview
// section is open *and* a book is loaded. A page change, a book change
// or closing the section cancels the render still running, and a token
// discards a result that arrives after the selection moved on.

const hasPdf = computed(() => (book.value?.formats ?? []).includes("pdf"));
const previewCanvas = ref<HTMLCanvasElement | null>(null);
const previewPage = ref(1);
const previewCount = ref(0);
const previewBusy = ref(false);
const previewError = ref<string | null>(null);

let previewDoc: { id: number; promise: Promise<PDFDocumentProxy> } | null = null;
let previewToken = 0;
let previewTask: { cancel: () => void } | null = null;

function previewDocument(id: number): Promise<PDFDocumentProxy> {
  if (previewDoc && previewDoc.id === id) return previewDoc.promise;
  dropPreviewDoc();
  const promise = loadPdfJs().then((pdfjs) => pdfjs.getDocument({ url: pdfUrl(String(id)), ...PDF_ASSET_OPTIONS }).promise);
  previewDoc = { id, promise };
  promise.catch(() => {
    if (previewDoc?.promise === promise) previewDoc = null;
  });
  return promise;
}

function dropPreviewDoc() {
  previewTask?.cancel();
  previewTask = null;
  const old = previewDoc;
  previewDoc = null;
  void old?.promise.then((d) => d.destroy()).catch(() => undefined);
}

function isCancel(e: unknown): boolean {
  return e instanceof Error && e.name === "RenderingCancelledException";
}

async function renderPreview() {
  const id = props.bookId;
  const token = ++previewToken;
  previewTask?.cancel();
  previewTask = null;
  if (!sections.value.preview || !book.value || book.value.id !== id || !hasPdf.value) return;
  previewBusy.value = true;
  previewError.value = null;
  try {
    const doc = await previewDocument(id);
    if (token !== previewToken) return;
    previewCount.value = doc.numPages;
    const pageNo = Math.min(Math.max(1, previewPage.value), doc.numPages);
    await nextTick();
    const canvas = previewCanvas.value;
    if (!canvas || token !== previewToken) return;
    const first = await doc.getPage(pageNo);
    const width = canvas.parentElement?.clientWidth || 320;
    const scale = scaleForWidth(first.getViewport({ scale: 1 }).width, width);
    if (token !== previewToken) return;
    await renderPage(doc, pageNo, { canvas, scale, onTask: (t) => (previewTask = t) });
  } catch (e) {
    if (!isCancel(e) && token === previewToken) previewError.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (token === previewToken) previewBusy.value = false;
  }
}

function previewGo(delta: number) {
  const next = Math.min(Math.max(1, previewPage.value + delta), Math.max(1, previewCount.value));
  if (next !== previewPage.value) previewPage.value = next;
}

watch(
  () => [sections.value.preview, book.value?.id, previewPage.value] as const,
  () => void renderPreview(),
);
// A different book starts at its first page and drops the old document.
watch(
  () => props.bookId,
  () => {
    previewPage.value = 1;
    previewCount.value = 0;
    dropPreviewDoc();
    closeViewer();
  },
);
// Closing the section stops any render in flight.
watch(
  () => sections.value.preview,
  (open) => {
    if (!open) {
      previewToken++;
      previewTask?.cancel();
    }
  },
);
onBeforeUnmount(() => {
  previewToken++;
  dropPreviewDoc();
  closeViewer();
});

// The viewer shows the cover, or the current PDF page rendered large.
const viewer = ref<null | "cover" | "preview">(null);
const pageImageUrl = ref("");
const pageImageBusy = ref(false);
const VIEWER_PAGE_WIDTH = 2400;

function revokePageImage() {
  if (pageImageUrl.value) URL.revokeObjectURL(pageImageUrl.value);
  pageImageUrl.value = "";
}

async function renderViewerPage() {
  const id = props.bookId;
  const pageNo = previewPage.value;
  pageImageBusy.value = true;
  try {
    const doc = await previewDocument(id);
    const blob = await renderPageToBlob(doc, pageNo, VIEWER_PAGE_WIDTH, "png");
    if (viewer.value !== "preview" || id !== props.bookId || pageNo !== previewPage.value) return;
    revokePageImage();
    pageImageUrl.value = URL.createObjectURL(blob);
  } catch (e) {
    previewError.value = e instanceof Error ? e.message : String(e);
  } finally {
    pageImageBusy.value = false;
  }
}

function openCoverViewer() {
  if (book.value) viewer.value = "cover";
}

function openPreviewViewer() {
  if (!hasPdf.value) return;
  viewer.value = "preview";
  void renderViewerPage();
}

function closeViewer() {
  viewer.value = null;
  revokePageImage();
}

watch(previewPage, () => {
  if (viewer.value === "preview") void renderViewerPage();
});

function onViewerStep(delta: number) {
  if (viewer.value === "preview") previewGo(delta);
  else emit("step-book", delta);
}

const viewerSrc = computed(() => (viewer.value === "preview" ? pageImageUrl.value : coverSrc.value));
const viewerCaption = computed(() => (viewer.value === "preview" ? `Page ${previewPage.value} of ${previewCount.value || "?"}` : undefined));

// Data files load once the Files section is open.
watch(
  () => [sections.value.files, book.value?.id] as const,
  ([open, id]) => {
    if (open && id !== undefined) {
      void loadDataFiles();
      void loadFormatSizes();
    }
  },
  { immediate: true },
);

function read() {
  if (!readableFormat.value) return;
  void router.push({ name: "read", params: { bookId: String(props.bookId), fmt: readableFormat.value } });
}

/** Custom columns the form cannot edit (computed columns): shown read-only. */
const readOnlyCustomValues = computed(() => {
  const b = book.value;
  if (!b) return [];
  return customColumnFields.value.filter((f) => !f.is_editable).map((f) => ({ field: f, value: b[f.label] })).filter((v) => v.value !== null && v.value !== undefined && v.value !== "");
});

// ---------------------------------------------------------------
// Right-click menu dispatch (#1.2)
// ---------------------------------------------------------------
//
// The panel already owns every book-scoped action; the context menu
// just needs a way to ask for one. Rather than duplicating those
// controls, the menu selects the book and names an action, and this
// performs it once the book has loaded.
//
// Unknown ids are ignored on purpose: the registry can list an action
// before this panel implements it, and a menu entry that does nothing
// is better than a crash. The menu itself only offers ids the view
// declares as handled, so this should not happen in practice.
function performAction(id: string) {
  switch (id) {
    case "read":
      read();
      break;
    case "edit-metadata":
      startEditing();
      break;
    case "fetch-metadata":
      fetchMetadataOpen.value = true;
      break;
    case "convert":
      void openConvert();
      break;
    case "tweak-book":
      if (canTweak.value) tweakOpen.value = true;
      break;
    case "quick-view":
      openQuickView();
      break;
    case "test-template":
      templateTesterOpen.value = true;
      break;
    case "send-email":
      openShare();
      break;
    case "replace-cover":
      coverInput.value?.click();
      break;
    case "cover-from-pdf":
      void coverFromPdf();
      break;
    case "open-externally":
      void openExternally();
      break;
    case "open-book-folder":
      void revealBookFolder();
      break;
    case "unpack-book":
      void unpackBook();
      break;
    case "repack-book":
      void repackBook();
      break;
    default:
      break;
  }
}

// Waits for the book itself: several of these actions read `book`,
// and the panel mounts before the fetch resolves.
watch(
  () => [props.pendingAction?.nonce, book.value?.id] as const,
  () => {
    const pending = props.pendingAction;
    if (pending && book.value) performAction(pending.id);
  },
  { immediate: true },
);
</script>

<template>
  <!-- Docked it is the details column; otherwise a modal Dialog around the same panel. -->
  <component :is="docked ? 'div' : Dialog" v-bind="docked ? { class: 'docked-host' } : { size: 'md', label: 'Book details', onClose: () => emit('close') }">
    <div ref="rootEl" class="panel" :class="{ docked }">
      <button class="close" :title="docked ? 'Hide details' : 'Close'" :aria-label="docked ? 'Hide details' : 'Close details'" @click="emit('close')">✕</button>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else-if="book && form">
        <div class="title-block">
          <h2>{{ book.title }}</h2>
          <p class="authors">{{ (book.authors ?? []).join(" & ") }}</p>
          <p v-if="book.series" class="series">{{ book.series }} #{{ book.series_index }}</p>
          <p v-if="book.rating" class="rating">{{ "★".repeat(Math.round(book.rating)) }}</p>
        </div>
        <p v-if="deleteError" class="error">{{ deleteError }}</p>
        <p v-if="openExternallyError" class="error">{{ openExternallyError }}</p>
        <p v-if="unpackMessage" class="status">{{ unpackMessage }}</p>

        <CollapsibleSection id="cover" title="Cover" :open="sections.cover" @toggle="toggle('cover')">
          <button type="button" class="cover-button" title="Click to enlarge" aria-label="Enlarge the cover" @click="openCoverViewer">
            <img :src="coverSrc" :alt="`Cover of ${book.title}`" class="cover" />
          </button>
          <div class="row">
            <button type="button" class="edit" @click="coverInput?.click()">Replace cover…</button>
            <button v-if="canCoverFromPdf" type="button" class="edit" :disabled="saving" title="Render the first page of the PDF as the cover" @click="coverFromPdf">Cover from page 1</button>
            <input ref="coverInput" type="file" accept="image/jpeg,image/png" class="hidden-file-input" @change="replaceCover" />
          </div>
        </CollapsibleSection>

        <CollapsibleSection id="preview" title="Preview" :open="sections.preview" @toggle="toggle('preview')">
          <template v-if="hasPdf">
            <div class="row preview-nav">
              <button type="button" class="edit" :disabled="previewPage <= 1" aria-label="Previous page" @click="previewGo(-1)">‹</button>
              <label class="page-box">
                Page
                <input type="number" min="1" :max="Math.max(1, previewCount)" :value="previewPage" aria-label="Page number" @change="previewPage = Math.min(Math.max(1, Number(($event.target as HTMLInputElement).value) || 1), Math.max(1, previewCount))" />
                <span v-if="previewCount">of {{ previewCount }}</span>
              </label>
              <button type="button" class="edit" :disabled="previewPage >= previewCount" aria-label="Next page" @click="previewGo(1)">›</button>
            </div>
            <div class="preview-frame">
              <canvas ref="previewCanvas" class="preview-canvas" title="Click to enlarge" @click="openPreviewViewer"></canvas>
              <p v-if="previewBusy" class="hint">Rendering…</p>
            </div>
            <p v-if="previewError" class="error">{{ previewError }}</p>
          </template>
          <template v-else>
            <p class="hint">No preview for {{ (book.formats[0] ?? "this book").toUpperCase() }} yet.</p>
            <button type="button" class="cover-button" title="Click to enlarge" @click="openCoverViewer"><img :src="coverSrc" alt="" class="cover" /></button>
          </template>
        </CollapsibleSection>

        <CollapsibleSection id="metadata" title="Metadata" :open="sections.metadata" @toggle="toggle('metadata')">
          <template #head><span v-if="dirty" class="dirty-mark">unsaved</span></template>
          <form class="meta-form" @submit.prevent="saveEdits">
            <div class="fields">
              <label>Title <input v-model="form.title" data-field="title" required /></label>
              <label>Authors <AutocompleteInput v-model="form.authors" data-field="authors" field="authors" separator="&" placeholder="Jane Doe & John Smith" /></label>
              <label>Series <AutocompleteInput v-model="form.series" data-field="series" field="series" /></label>
              <label>Series index <input v-model="form.seriesIndex" data-field="series_index" type="number" step="0.1" /></label>
              <label>Tags <AutocompleteInput v-model="form.tags" data-field="tags" field="tags" separator="," placeholder="scifi, classic" /></label>
              <label>Rating <input v-model.number="form.rating" data-field="rating" type="number" min="0" max="5" step="1" /></label>
              <label>Publisher <AutocompleteInput v-model="form.publisher" data-field="publisher" field="publisher" /></label>
              <label>Published <input v-model="form.pubdate" data-field="pubdate" type="date" /></label>
              <label class="edit-comments">Comments <textarea v-model="form.comments" data-field="comments" rows="4"></textarea></label>
              <template v-for="field in customColumnFields" :key="field.key">
                <label v-if="field.is_editable && field.datatype === 'bool'" class="checkbox-field">
                  <input type="checkbox" :checked="form.custom[field.label] === 'true'" @change="form.custom[field.label] = ($event.target as HTMLInputElement).checked ? 'true' : 'false'" />
                  {{ field.name ?? field.label }}
                </label>
                <label v-else-if="field.is_editable && (field.datatype === 'int' || field.datatype === 'float' || field.datatype === 'rating')">
                  {{ field.name ?? field.label }}
                  <input v-model="form.custom[field.label]" :data-field="field.label" type="number" :step="field.datatype === 'int' ? 1 : 0.1" />
                </label>
                <label v-else-if="field.is_editable">
                  {{ field.name ?? field.label }}
                  <AutocompleteInput v-if="completionForCustom(field) || enumValuesOf(field)" v-model="form.custom[field.label]" :data-field="field.label" :field="completionForCustom(field)?.field" :separator="completionForCustom(field)?.separator" :options="enumValuesOf(field) ?? undefined" :min-chars="enumValuesOf(field) ? 1 : undefined" />
                  <input v-else v-model="form.custom[field.label]" :data-field="field.label" />
                </label>
              </template>
            </div>
            <p v-if="saveError" class="error">{{ saveError }}</p>
            <div class="row">
              <button type="submit" class="read primary" data-action="save" :disabled="saving || !dirty">{{ saving ? "Saving…" : "Save" }}</button>
              <button type="button" class="edit" data-action="revert" :disabled="saving || !dirty" @click="revert">Revert</button>
              <button type="button" class="edit" @click="fetchMetadataOpen = true">Fetch metadata online…</button>
            </div>
          </form>
          <dl v-if="readOnlyCustomValues.length" class="custom-columns">
            <template v-for="{ field, value } in readOnlyCustomValues" :key="field.key">
              <dt>{{ field.name ?? field.label }}</dt>
              <dd>{{ value }}</dd>
            </template>
          </dl>
        </CollapsibleSection>

        <CollapsibleSection id="files" title="Files" :open="sections.files" @toggle="toggle('files')">
          <ul class="file-list">
            <li v-for="[fmt, url] in formatLinks" :key="fmt" class="file-row" :data-format="fmt">
              <span class="file-name">{{ fmt.toUpperCase() }}</span>
              <span class="file-size">{{ formatSizes[fmt] ? humanSize(formatSizes[fmt]) : "" }}</span>
              <span class="file-actions">
                <button v-if="desktop" type="button" class="edit small" data-action="show-in-folder" title="Show this file in the file manager" @click="revealFile(fmt)">Show in folder</button>
                <button v-if="desktop" type="button" class="edit small" :disabled="openingExternally" @click="openFormat(fmt)">Open</button>
                <a :href="url" class="download small" :download="`${book.title}.${fmt}`">Download</a>
                <button type="button" class="format-chip-remove" :disabled="formatBusy" :aria-label="`Remove ${fmt.toUpperCase()}`" title="Remove this format" @click="removeFormatClick(fmt)">✕</button>
              </span>
            </li>
          </ul>
          <p v-if="formatLinks.length === 0" class="hint">This book has no files.</p>
          <div class="row">
            <button type="button" class="edit small" :disabled="formatBusy" @click="formatInput?.click()">{{ formatBusy ? "Working…" : "Add format…" }}</button>
            <input ref="formatInput" type="file" class="hidden-file-input" @change="addFormatFile" />
          </div>
          <div v-if="book.path" class="folder-path">
            <span class="hint">Folder</span>
            <code class="path-text" data-testid="folder-path">{{ book.path }}</code>
            <button type="button" class="edit small" @click="copyFolderPath">{{ copied ? "Copied" : "Copy" }}</button>
            <button v-if="desktop" type="button" class="edit small" data-action="show-folder" @click="revealBookFolder">Show in folder</button>
          </div>
          <div class="format-manager">
            <span class="format-manager-label">Data files</span>
            <span v-if="dataFilesLoading" class="hint">Loading…</span>
            <span v-for="[relpath, stat] in Object.entries(dataFiles)" :key="relpath" class="format-chip" :title="`${stat.size} bytes`">
              {{ dataFileName(relpath) }}
              <button type="button" class="format-chip-remove" :disabled="dataFileBusy" :aria-label="`Remove ${dataFileName(relpath)}`" @click="removeDataFileClick(relpath)">✕</button>
            </span>
            <button type="button" class="edit small" :disabled="dataFileBusy" @click="dataFileInput?.click()">{{ dataFileBusy ? "Working…" : "Add data file…" }}</button>
            <input ref="dataFileInput" type="file" class="hidden-file-input" @change="addDataFile" />
          </div>
        </CollapsibleSection>

        <CollapsibleSection id="actions" title="Actions" :open="sections.actions" @toggle="toggle('actions')">
          <div class="actions-row">
            <button v-if="readableFormat" class="read primary" @click="read">Read ({{ readableFormat.toUpperCase() }})</button>
            <button class="edit" @click="openConvert">Convert…</button>
            <button class="edit" @click="openShare">Send…</button>
            <button v-if="canTweak" class="edit" @click="tweakOpen = true">Tweak Book…</button>
            <button class="edit" @click="openQuickView">Quick View…</button>
            <button class="edit" @click="templateTesterOpen = !templateTesterOpen">Test template…</button>
            <button v-if="canUnpack" class="edit" :disabled="unpacking" @click="unpackBook">{{ unpacking ? "Working…" : `Unpack ${unpackableFormat?.toUpperCase()}…` }}</button>
            <button v-if="canUnpack" class="edit" :disabled="unpacking" @click="repackBook">Repack…</button>
            <button class="delete danger" :disabled="deleting" title="Moves the book to the library Trash; it can be restored from there" @click="deleteBook">{{ deleting ? "Moving…" : "Move to Trash" }}</button>
          </div>

        <div v-if="templateTesterOpen" class="template-tester">
          <p class="hint">Template Program Mode only (e.g. <code>field('title')</code>) -- the <code>{field}</code> shorthand isn't supported yet.</p>
          <textarea v-model="templateInput" rows="2" spellcheck="false"></textarea>
          <div class="template-tester-actions">
            <button type="button" :disabled="templateRunning" @click="runTemplateTest">{{ templateRunning ? "Running…" : "Run" }}</button>
            <select v-if="savedTemplates.length" :value="''" @change="loadSavedTemplate(($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''">
              <option value="" disabled>Load saved…</option>
              <option v-for="t in savedTemplates" :key="t.name" :value="t.name">{{ t.name }}</option>
            </select>
            <input v-model="templateNameInput" placeholder="Save as…" :disabled="templateRunning" />
            <button type="button" :disabled="templateRunning || !templateNameInput.trim()" @click="saveCurrentTemplate">Save</button>
          </div>
          <p v-if="templateSaveError" class="error">{{ templateSaveError }}</p>
          <ul v-if="savedTemplates.length" class="saved-template-list">
            <li v-for="t in savedTemplates" :key="t.name">
              <button type="button" class="saved-template-name" @click="loadSavedTemplate(t.name)">{{ t.name }}</button>
              <code>{{ t.template }}</code>
              <button type="button" title="Delete this saved template" @click="deleteSavedTemplate(t.name)">✕</button>
            </li>
          </ul>
          <p v-if="templateResult !== null" class="template-result">{{ templateResult || "(empty result)" }}</p>
          <p v-if="templateError" class="error">{{ templateError }}</p>
        </div>

        <div v-if="quickViewOpen" class="quick-view">
          <p v-if="quickViewLoading">Loading…</p>
          <p v-else-if="quickViewGroups.length === 0" class="hint">No other books share this book's authors, tags, or series.</p>
          <div v-for="group in quickViewGroups" :key="group.label" class="quick-view-group">
            <h4>{{ group.label }}</h4>
            <ul>
              <li v-for="b in group.books" :key="b.id">
                <button type="button" @click="openBookFromQuickView(b.id)">{{ b.title }}</button>
              </li>
            </ul>
          </div>
        </div>

        <div v-if="convertOpen" class="convert-panel">
          <p v-if="convertLoadingFormats">Loading formats…</p>
          <template v-else>
            <div class="convert-row">
              <label>
                From
                <select v-model="convertInputFmt" :disabled="converting">
                  <option v-for="fmt in convertInputFormats" :key="fmt" :value="fmt">{{ fmt }}</option>
                </select>
              </label>
              <label>
                To
                <select v-model="convertOutputFmt" :disabled="converting">
                  <option v-for="fmt in convertOutputFormats" :key="fmt" :value="fmt">{{ fmt }}</option>
                </select>
              </label>
              <button type="button" class="read primary" :disabled="converting || !convertInputFmt || !convertOutputFmt" @click="runConversion">
                {{ converting ? "Converting…" : "Start" }}
              </button>
              <button type="button" :disabled="converting" @click="convertOpen = false">Close</button>
            </div>
            <details class="convert-options">
              <summary>Options</summary>
              <div class="convert-options-grid">
                <label><input type="checkbox" v-model="convertUnsmartenPunctuation" :disabled="converting" /> Convert smart quotes/dashes to plain ASCII</label>
                <label><input type="checkbox" v-model="convertLinearizeTables" :disabled="converting" /> Linearize tables</label>
                <label><input type="checkbox" v-model="convertInsertMetadata" :disabled="converting" /> Insert a metadata jacket page</label>
                <label><input type="checkbox" v-model="convertRemoveFirstImage" :disabled="converting" /> Remove the first image (if it's a cover)</label>
                <label><input type="checkbox" v-model="convertUseAutoToc" :disabled="converting" /> Force auto-generated table of contents</label>
                <label class="convert-options-text">
                  Chapter detection XPath
                  <input type="text" v-model="convertChapter" :disabled="converting" placeholder="//h:h1 | //h:h2" />
                </label>
                <label class="convert-options-text">
                  Max TOC links
                  <input type="number" min="0" v-model="convertMaxTocLinks" :disabled="converting" />
                </label>
                <label class="convert-options-text">
                  Base font size (pt)
                  <input type="number" min="0" step="0.5" v-model="convertBaseFontSize" :disabled="converting" />
                </label>
              </div>
            </details>
            <p v-if="convertDone" class="convert-done">Converted to {{ convertOutputFmt }} -- format added to this book.</p>
          </template>
          <p v-if="convertError" class="error">{{ convertError }}</p>
        </div>

        <form v-if="shareOpen" class="convert-panel" @submit.prevent="sendShareEmail">
          <div class="convert-row">
            <label>
              Format
              <select v-model="shareFormat" :disabled="sharing">
                <option v-for="fmt in book.formats" :key="fmt" :value="fmt">{{ fmt.toUpperCase() }}</option>
              </select>
            </label>
            <label>
              From
              <input v-model="shareFrom" type="email" placeholder="me@example.com" required :disabled="sharing" />
            </label>
            <label>
              To
              <input v-model="shareTo" type="email" placeholder="you@example.com" required :disabled="sharing" />
            </label>
          </div>
          <div class="convert-row">
            <label>
              SMTP relay
              <input v-model="shareRelay.relay" placeholder="smtp.example.com" required :disabled="sharing" />
            </label>
            <label>
              Port
              <input v-model.number="shareRelay.port" type="number" placeholder="587" :disabled="sharing" />
            </label>
            <label>
              Encryption
              <select v-model="shareRelay.encryption" :disabled="sharing">
                <option value="tls">STARTTLS</option>
                <option value="ssl">SSL</option>
                <option value="none">None</option>
              </select>
            </label>
          </div>
          <div class="convert-row">
            <label>
              Username
              <input v-model="shareRelay.username" :disabled="sharing" />
            </label>
            <label>
              Password
              <input v-model="shareRelay.password" type="password" :disabled="sharing" />
            </label>
          </div>
          <div class="convert-row">
            <button type="submit" class="read primary" :disabled="sharing">{{ sharing ? "Sending…" : "Send" }}</button>
            <button type="button" :disabled="sharing" @click="shareOpen = false">Close</button>
          </div>
          <p v-if="shareDone" class="convert-done">Sent.</p>
          <p v-if="shareError" class="error">{{ shareError }}</p>
        </form>
        </CollapsibleSection>
      </template>
    </div>
</component>
  <ImageViewer v-if="viewer" :src="viewerSrc" :title="book?.title ?? ''" :caption="viewerCaption" :loading="pageImageBusy" steppable @close="closeViewer" @step="onViewerStep" />
  <TweakEditor v-if="tweakOpen" :book-id="bookId" @close="tweakOpen = false" @updated="onTweakUpdated" />
  <FetchMetadataDialog v-if="fetchMetadataOpen" :book-id="bookId" :book="book" @close="fetchMetadataOpen = false" @updated="onFetchMetadataUpdated" />
</template>


<style scoped>
.panel {
  position: relative;
}
/* Docked: the panel *is* the column, so the modal's centring, rounding
   and width caps all have to come back off. */
.docked-host {
  display: flex;
  height: 100%;
  min-height: 0;
}
.panel.docked {
  background: var(--bg);
  width: 100%;
  height: 100%;
  padding: 1em;
  overflow-y: auto;
}
.close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: var(--fs-medium);
  cursor: pointer;
}
.header {
  display: flex;
  gap: 1em;
}
.cover {
  /* Fits the panel width; the panel itself is resizable. */
  display: block;
  width: 100%;
  max-height: 45vh;
  object-fit: contain;
}
.cover-button {
  all: unset;
  display: block;
  width: 100%;
  cursor: zoom-in;
}
.cover-button:focus-visible {
  outline: 2px solid var(--accent);
}
.title-block h2 {
  margin: 0 1.5rem 0.2rem 0;
  font-size: var(--fs-large, 1.15em);
}
.title-block p {
  margin: 0.1rem 0;
}
.row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4em;
  margin-top: 0.5em;
  align-items: center;
}
.preview-frame {
  margin-top: 0.5em;
  width: 100%;
  overflow: hidden;
}
.preview-canvas {
  display: block;
  max-width: 100%;
  cursor: zoom-in;
  border: 1px solid var(--border);
}
.page-box {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
  font-size: var(--fs-small);
}
.page-box input {
  width: 4em;
  font: inherit;
}
.meta-form .fields label {
  display: flex;
  flex-direction: column;
}
.meta-form .fields input,
.meta-form .fields textarea {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
}
.dirty-mark {
  font-size: var(--fs-small);
  color: var(--accent);
}
.file-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.file-row {
  display: flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.25em 0;
  flex-wrap: wrap;
}
.file-name {
  font-weight: 600;
  min-width: 3.5em;
}
.file-size {
  color: var(--fg-faint);
  font-size: var(--fs-small);
}
.file-actions {
  margin-left: auto;
  display: flex;
  gap: 0.3em;
  align-items: center;
  flex-wrap: wrap;
}
.small {
  padding: 0.2em 0.55em !important;
  font-size: var(--fs-small);
}
.folder-path {
  display: flex;
  gap: 0.4em;
  align-items: center;
  flex-wrap: wrap;
  margin-top: 0.6em;
}
.path-text {
  flex: 1;
  min-width: 0;
  word-break: break-all;
  font-size: var(--fs-small);
}
.actions-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4em;
}
.actions-row .delete {
  margin-left: 0;
}
.authors {
  color: var(--fg-muted);
}
.tags {
  color: var(--fg-faint);
  font-size: var(--fs-body);
}
.custom-columns {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 0.2em 0.75em;
  margin: 0.5em 0 0;
  font-size: var(--fs-body);
}
.custom-columns dt {
  color: var(--fg-faint);
}
.custom-columns dd {
  margin: 0;
}
.formats {
  margin-top: 1em;
  display: flex;
  gap: 0.5em;
  flex-wrap: wrap;
}
/* Color, border and the disabled look come from the global `button.primary` (and the
   Classic skin's rules for it): a flat borderless button here left a disabled Save looking
   enabled, at 2.2:1 contrast in Classic. */
.read {
  padding: 0.5em 1em;
}
.download {
  border: 1px solid var(--border);
  padding: 0.5em 1em;
  border-radius: var(--radius-md);
  text-decoration: none;
  color: inherit;
}
.error {
  color: var(--danger);
}
.edit {
  background: none;
  border: 1px solid var(--border);
  padding: 0.5em 1em;
  border-radius: var(--radius-md);
  cursor: pointer;
}
.delete {
  background: none;
  border: 1px solid var(--danger);
  color: var(--danger);
  padding: 0.5em 1em;
  border-radius: var(--radius-md);
  cursor: pointer;
  margin-left: auto;
}
.delete:disabled {
  opacity: 0.6;
  cursor: default;
}
.edit-form .header {
  align-items: flex-start;
}
.cover-edit {
  display: flex;
  flex-direction: column;
  gap: 0.4em;
  width: 90px;
  flex-shrink: 0;
}
.hidden-file-input {
  display: none;
}
.fields {
  display: flex;
  flex-direction: column;
  gap: 0.5em;
  flex: 1;
}
.fields label {
  display: flex;
  flex-direction: column;
  font-size: var(--fs-small);
  color: var(--fg-muted);
  gap: 0.2em;
}
.fields input {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
}
.checkbox-field {
  flex-direction: row !important;
  align-items: center;
  gap: 0.4em !important;
}
.format-manager {
  margin-top: 1em;
  padding-top: 1em;
  border-top: 1px solid var(--border);
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 0.5em;
}
.format-manager-label {
  font-size: var(--fs-small);
  color: var(--fg-muted);
  font-weight: 600;
}
.hint {
  font-size: var(--fs-small);
  color: var(--fg-faint);
}
.template-tester {
  border-top: 1px solid var(--border);
  padding-top: 0.75em;
  margin-top: 0.5em;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}
.template-tester textarea {
  font-family: ui-monospace, monospace;
  font-size: var(--fs-small);
  padding: 0.4em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  resize: vertical;
}
.template-tester-actions {
  display: flex;
}
.template-result {
  font-family: ui-monospace, monospace;
  font-size: var(--fs-small);
  background: var(--bg-bar);
  padding: 0.4em 0.6em;
  border-radius: var(--radius-md);
  word-break: break-word;
}
.quick-view {
  border-top: 1px solid var(--border);
  padding-top: 0.75em;
  margin-top: 0.5em;
  display: flex;
  flex-direction: column;
  gap: 0.75em;
}
.quick-view-group h4 {
  margin: 0 0 0.3em;
  font-size: var(--fs-small);
  color: var(--fg-muted);
}
.quick-view-group ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 0.4em;
}
.quick-view-group button {
  background: none;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.3em 0.6em;
  font: inherit;
  font-size: var(--fs-small);
  cursor: pointer;
}
.format-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.3em;
  background: var(--bg-sunken);
  border-radius: var(--radius-md);
  padding: 0.3em 0.5em;
  font-size: var(--fs-small);
}
.format-chip-remove {
  border: none;
  background: none;
  cursor: pointer;
  color: var(--fg-faint);
  font-size: var(--fs-body);
  padding: 0;
  line-height: 1;
}
.format-chip-remove:disabled {
  opacity: 0.5;
  cursor: default;
}
.convert-panel {
  margin-top: 1em;
  padding: 0.9em;
  background: var(--bg-sunken);
  border-radius: var(--radius-lg);
}
.convert-row {
  display: flex;
  align-items: flex-end;
  gap: 0.75em;
  flex-wrap: wrap;
}
.convert-row label {
  display: flex;
  flex-direction: column;
  font-size: var(--fs-small);
  color: var(--fg-muted);
  gap: 0.2em;
}
.convert-row select {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
}
.convert-done {
  color: var(--success);
  margin: 0.6em 0 0;
}
.convert-options {
  margin-top: 0.6em;
  font-size: var(--fs-small);
}
.convert-options summary {
  cursor: pointer;
  color: var(--fg-muted);
}
.convert-options-grid {
  display: flex;
  flex-direction: column;
  gap: 0.4em;
  margin-top: 0.5em;
}
.convert-options-grid label {
  display: flex;
  align-items: center;
  gap: 0.4em;
  color: var(--fg);
}
.convert-options-text {
  flex-direction: column !important;
  align-items: flex-start !important;
}
.convert-options-text input[type="text"],
.convert-options-text input[type="number"] {
  font: inherit;
  padding: 0.3em 0.45em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  width: 100%;
  max-width: 20em;
}
/* Saved templates (#1.15). */
.saved-template-list {
  list-style: none;
  margin: 0.4rem 0 0;
  padding: 0;
  font-size: var(--fs-small);
}
.saved-template-list li {
  display: flex;
  gap: 0.5rem;
  align-items: center;
  padding: 0.15rem 0;
}
.saved-template-name {
  all: unset;
  cursor: pointer;
  font-weight: 600;
  white-space: nowrap;
}
.saved-template-name:hover {
  text-decoration: underline;
}
.saved-template-list code {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  opacity: 0.75;
}

</style>
