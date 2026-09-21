<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import FetchMetadataDialog from "./FetchMetadataDialog.vue";
import TweakEditor from "./TweakEditor.vue";
import { addFormat, deleteBooks, evaluateTemplate, fetchBook, fetchBooks, fetchConversionBookData, fetchDataFiles, fetchFieldMetadata, getConversionStatus, removeDataFile, removeFormat, search, setCover, setFields, shareEmail, startConversion, uploadDataFile } from "../library/api";
import type { ConversionOptionsOverride, DataFileStat, SmtpRelayConfig } from "../library/api";
import { categoryItemToQuery } from "../library/query";
import { isTauri, tauriInvoke } from "../tauri";
import type { BookFieldChanges, BookSummary, FieldMetaEntry } from "../library/types";

const props = defineProps<{
  bookId: number;
  /**
   * An action to perform as soon as the book has loaded -- how the
   * right-click menu (#1.2) reaches the panel's own controls. A
   * counter accompanies it so choosing the *same* action twice in a
   * row still fires: watching the id alone would see no change.
   */
  pendingAction?: { id: string; nonce: number } | null;
}>();
const emit = defineEmits<{ close: []; updated: []; deleted: [bookId: number]; "open-book": [bookId: number] }>();
const router = useRouter();

const book = ref<BookSummary | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);

const deleting = ref(false);
const deleteError = ref<string | null>(null);

async function deleteBook() {
  const b = book.value;
  if (!b) return;
  if (!confirm(`Delete "${b.title}"? This cannot be undone.`)) return;
  deleting.value = true;
  deleteError.value = null;
  try {
    await deleteBooks([props.bookId]);
    emit("deleted", props.bookId);
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
// options -- see crates/calibre_srv/src/convert.rs's own doc for why.
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
// server-side yet (crates/calibre_srv/src/share.rs's own doc: ties
// into the not-yet-built preferences epic), so the relay config is
// remembered client-side in localStorage instead, purely for this
// browser's own convenience across sends.
const RELAY_STORAGE_KEY = "calibre-oxide-smtp-relay";

function loadSavedRelay(): SmtpRelayConfig {
  try {
    const raw = localStorage.getItem(RELAY_STORAGE_KEY);
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

const editing = ref(false);
const saving = ref(false);
const saveError = ref<string | null>(null);
const editTitle = ref("");
const editAuthors = ref("");
const editSeries = ref("");
const editSeriesIndex = ref("");
const editTags = ref("");
const editRating = ref(0);
const coverInput = ref<HTMLInputElement | null>(null);

// Custom columns (issue #720) -- discovered from /ajax/field-metadata
// (real, already includes custom columns, see that route's own doc),
// rendered/edited generically alongside the fixed fields above. `key`
// is "#label" (field-metadata's own namespacing so a custom column
// can't collide with a standard one); `label` is the bare name that
// both the book row (Cache::get_data_as_dict) and set-fields'
// `changes` object actually use.
const customColumnFields = ref<FieldMetaEntry[]>([]);
const editCustomValues = ref<Record<string, string>>({});

async function loadCustomColumnFields() {
  try {
    const fm = await fetchFieldMetadata();
    customColumnFields.value = Object.values(fm.field_metadata).filter((f) => f.is_custom);
  } catch (e) {
    console.error("failed to load custom column metadata", e);
  }
}
void loadCustomColumnFields();

function startEditing() {
  const b = book.value;
  if (!b) return;
  editTitle.value = b.title;
  editAuthors.value = (b.authors ?? []).join(" & ");
  editSeries.value = b.series ?? "";
  editSeriesIndex.value = b.series_index != null ? String(b.series_index) : "";
  editTags.value = (b.tags ?? []).join(", ");
  editRating.value = b.rating ?? 0;
  editCustomValues.value = {};
  for (const field of customColumnFields.value) {
    const raw = b[field.label];
    // `Cache::get_custom_column_value` reads a real bool column back
    // as "0"/"1" (an integer column under the hood), never
    // "true"/"false" -- normalize here so the checkbox's `checked`
    // binding below (which only recognizes the literal "true") works
    // for a value that actually came from the server, not just one
    // this form itself just wrote.
    if (field.datatype === "bool") {
      editCustomValues.value[field.label] = raw === "1" || raw === "true" || raw === true ? "true" : "false";
    } else {
      editCustomValues.value[field.label] = raw == null ? "" : String(raw);
    }
  }
  saveError.value = null;
  editing.value = true;
  void loadDataFiles();
}

async function saveEdits() {
  saving.value = true;
  saveError.value = null;
  try {
    const changes: BookFieldChanges = {
      title: editTitle.value,
      authors: editAuthors.value.split("&").map((a) => a.trim()).filter(Boolean),
      series: editSeries.value,
      tags: editTags.value.split(",").map((t) => t.trim()).filter(Boolean),
      rating: editRating.value,
    };
    if (editSeriesIndex.value.trim() !== "") {
      const idx = Number(editSeriesIndex.value);
      if (!Number.isNaN(idx)) changes.series_index = idx;
    }
    for (const field of customColumnFields.value) {
      if (!field.is_editable) continue;
      changes[field.label] = editCustomValues.value[field.label] ?? "";
    }
    book.value = await setFields(props.bookId, changes);
    editing.value = false;
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

async function replaceCover(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  saving.value = true;
  saveError.value = null;
  try {
    await setCover(props.bookId, file);
    book.value = await fetchBook(props.bookId);
    emit("updated");
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
    if (coverInput.value) coverInput.value.value = "";
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
  if (!confirm(`Remove the ${ext.toUpperCase()} format from this book?`)) return;
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
  if (!confirm(`Remove "${dataFileName(relpath)}" from this book?`)) return;
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

// Only formats the reader MVP (#499) actually round-trips through
// render_book are offered a "Read" link -- other formats still get a
// plain download link.
const READABLE_FORMATS = ["epub", "kepub"];

const readableFormat = computed(() => book.value?.formats.find((f) => READABLE_FORMATS.includes(f)) ?? null);

// "Open externally" (#818). The in-app reader handles EPUB/KEPUB only
// (`is_viewable_format` in calibre_srv), so for a PDF-first library
// this is the only way to actually open a book -- and it stays useful
// for any format the reader will never render.
//
// Desktop-only: handing a file to the OS default application is
// exactly what a browser tab cannot do.
const canOpenExternally = computed(() => isTauri() && (book.value?.formats.length ?? 0) > 0);
const openingExternally = ref(false);
const openExternallyError = ref<string | null>(null);

/// Prefers a format the in-app reader *cannot* show: if a book has
/// both an EPUB and a PDF, "Read" already covers the EPUB, so the
/// useful thing to hand to the system viewer is the PDF.
const externalFormat = computed(() => {
  const formats = book.value?.formats ?? [];
  return formats.find((f) => !READABLE_FORMATS.includes(f)) ?? formats[0] ?? null;
});

async function openExternally() {
  const fmt = externalFormat.value;
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

// Tweak Book (issue #719) -- crates/calibre_srv/src/tweak.rs's own
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
  editing.value = false;
  convertOpen.value = false;
  shareOpen.value = false;
  quickViewOpen.value = false;
  quickViewGroups.value = [];
  templateTesterOpen.value = false;
  templateResult.value = null;
  templateError.value = null;
  try {
    book.value = await fetchBook(id);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
watch(() => props.bookId, load, { immediate: true });

// The cover/thumbnail URLs are stable paths (`/get/cover/{id}`) --
// after replacing the cover the browser would otherwise keep showing
// the cached old image. Bust the cache with a query param that
// changes only when the book data itself was just reloaded.
const coverVersion = ref(0);
watch(book, () => coverVersion.value++);
const coverSrc = computed(() => (book.value ? `${book.value.thumbnail}?v=${coverVersion.value}` : ""));

function read() {
  if (!readableFormat.value) return;
  void router.push({ name: "read", params: { bookId: String(props.bookId), fmt: readableFormat.value } });
}

const visibleCustomColumnValues = computed(() => {
  const b = book.value;
  if (!b) return [];
  return customColumnFields.value.map((f) => ({ field: f, value: b[f.label] })).filter((v) => v.value !== null && v.value !== undefined && v.value !== "");
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
    case "open-externally":
      void openExternally();
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
  <div class="backdrop" @click.self="emit('close')">
    <div class="panel">
      <button class="close" @click="emit('close')">✕</button>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else-if="book && !editing">
        <div class="header">
          <img :src="coverSrc" alt="" class="cover" />
          <div>
            <h2>{{ book.title }}</h2>
            <p class="authors">{{ (book.authors ?? []).join(" & ") }}</p>
            <p v-if="book.series" class="series">{{ book.series }} #{{ book.series_index }}</p>
            <p v-if="book.rating" class="rating">{{ "★".repeat(Math.round(book.rating)) }}</p>
          </div>
        </div>

        <p v-if="(book.tags ?? []).length" class="tags">{{ (book.tags ?? []).join(", ") }}</p>
        <dl v-if="visibleCustomColumnValues.length" class="custom-columns">
          <template v-for="{ field, value } in visibleCustomColumnValues" :key="field.key">
            <dt>{{ field.name ?? field.label }}</dt>
            <dd>{{ field.datatype === "bool" ? (value === "1" || value === "true" ? "Yes" : "No") : value }}</dd>
          </template>
        </dl>
        <p v-if="deleteError" class="error">{{ deleteError }}</p>
        <p v-if="openExternallyError" class="error">{{ openExternallyError }}</p>

        <div class="formats">
          <button v-if="readableFormat" class="read" @click="read">Read ({{ readableFormat.toUpperCase() }})</button>
          <button v-if="canOpenExternally && externalFormat" class="edit" :disabled="openingExternally" @click="openExternally">
            {{ openingExternally ? "Opening…" : `Open ${externalFormat.toUpperCase()} externally` }}
          </button>
          <a v-for="[fmt, url] in formatLinks" :key="fmt" :href="url" class="download"> Download {{ fmt.toUpperCase() }} </a>
          <button class="edit" @click="startEditing">Edit metadata</button>
          <button class="edit" @click="fetchMetadataOpen = true">Fetch metadata online…</button>
          <button class="edit" @click="openConvert">Convert…</button>
          <button class="edit" @click="openShare">Send…</button>
          <button v-if="canTweak" class="edit" @click="tweakOpen = true">Tweak Book…</button>
          <button class="edit" @click="openQuickView">Quick View…</button>
          <button class="edit" @click="templateTesterOpen = !templateTesterOpen">Test template…</button>
          <button class="delete" :disabled="deleting" @click="deleteBook">{{ deleting ? "Deleting…" : "Delete" }}</button>
        </div>

        <div v-if="templateTesterOpen" class="template-tester">
          <p class="hint">Template Program Mode only (e.g. <code>field('title')</code>) -- the <code>{field}</code> shorthand isn't supported yet.</p>
          <textarea v-model="templateInput" rows="2" spellcheck="false"></textarea>
          <div class="template-tester-actions">
            <button type="button" :disabled="templateRunning" @click="runTemplateTest">{{ templateRunning ? "Running…" : "Run" }}</button>
          </div>
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
              <button type="button" class="read" :disabled="converting || !convertInputFmt || !convertOutputFmt" @click="runConversion">
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
            <button type="submit" class="read" :disabled="sharing">{{ sharing ? "Sending…" : "Send" }}</button>
            <button type="button" :disabled="sharing" @click="shareOpen = false">Close</button>
          </div>
          <p v-if="shareDone" class="convert-done">Sent.</p>
          <p v-if="shareError" class="error">{{ shareError }}</p>
        </form>
      </template>

      <form v-else-if="book && editing" class="edit-form" @submit.prevent="saveEdits">
        <div class="header">
          <div class="cover-edit">
            <img :src="coverSrc" alt="" class="cover" />
            <button type="button" @click="coverInput?.click()">Replace cover…</button>
            <input ref="coverInput" type="file" accept="image/jpeg,image/png" class="hidden-file-input" @change="replaceCover" />
          </div>
          <div class="fields">
            <label>Title <input v-model="editTitle" required /></label>
            <label>Authors <input v-model="editAuthors" placeholder="Jane Doe & John Smith" /></label>
            <label>Series <input v-model="editSeries" /></label>
            <label>Series index <input v-model="editSeriesIndex" type="number" step="0.1" /></label>
            <label>Tags <input v-model="editTags" placeholder="scifi, classic" /></label>
            <label>Rating <input v-model.number="editRating" type="number" min="0" max="5" step="1" /></label>
            <template v-for="field in customColumnFields" :key="field.key">
              <label v-if="field.is_editable && field.datatype === 'bool'" class="checkbox-field">
                <input type="checkbox" :checked="editCustomValues[field.label] === 'true'" @change="editCustomValues[field.label] = ($event.target as HTMLInputElement).checked ? 'true' : 'false'" />
                {{ field.name ?? field.label }}
              </label>
              <label v-else-if="field.is_editable && (field.datatype === 'int' || field.datatype === 'float' || field.datatype === 'rating')">
                {{ field.name ?? field.label }}
                <input v-model="editCustomValues[field.label]" type="number" :step="field.datatype === 'int' ? 1 : 0.1" />
              </label>
              <label v-else-if="field.is_editable">
                {{ field.name ?? field.label }}
                <input v-model="editCustomValues[field.label]" />
              </label>
            </template>
          </div>
        </div>

        <div class="format-manager">
          <span class="format-manager-label">Formats</span>
          <span v-for="[fmt] in formatLinks" :key="fmt" class="format-chip">
            {{ fmt.toUpperCase() }}
            <button type="button" class="format-chip-remove" :disabled="formatBusy" @click="removeFormatClick(fmt)" :aria-label="`Remove ${fmt.toUpperCase()}`">✕</button>
          </span>
          <button type="button" :disabled="formatBusy" @click="formatInput?.click()">{{ formatBusy ? "Working…" : "Add format…" }}</button>
          <input ref="formatInput" type="file" class="hidden-file-input" @change="addFormatFile" />
        </div>

        <div class="format-manager">
          <span class="format-manager-label">Data files</span>
          <span v-if="dataFilesLoading" class="hint">Loading…</span>
          <span v-for="[relpath, stat] in Object.entries(dataFiles)" :key="relpath" class="format-chip" :title="`${stat.size} bytes`">
            {{ dataFileName(relpath) }}
            <button type="button" class="format-chip-remove" :disabled="dataFileBusy" @click="removeDataFileClick(relpath)" :aria-label="`Remove ${dataFileName(relpath)}`">✕</button>
          </span>
          <button type="button" :disabled="dataFileBusy" @click="dataFileInput?.click()">{{ dataFileBusy ? "Working…" : "Add data file…" }}</button>
          <input ref="dataFileInput" type="file" class="hidden-file-input" @change="addDataFile" />
        </div>

        <p v-if="saveError" class="error">{{ saveError }}</p>

        <div class="formats">
          <button type="submit" class="read" :disabled="saving">{{ saving ? "Saving…" : "Save" }}</button>
          <button type="button" @click="editing = false" :disabled="saving">Cancel</button>
        </div>
      </form>
    </div>
  </div>
  <TweakEditor v-if="tweakOpen" :book-id="bookId" @close="tweakOpen = false" @updated="onTweakUpdated" />
  <FetchMetadataDialog v-if="fetchMetadataOpen" :book-id="bookId" :book="book" @close="fetchMetadataOpen = false" @updated="onFetchMetadataUpdated" />
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}
.panel {
  background: #fff;
  border-radius: 6px;
  padding: 1.5em;
  max-width: 520px;
  width: 90%;
  max-height: 85vh;
  overflow: auto;
  position: relative;
}
.close {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  border: none;
  background: none;
  font-size: 1.1em;
  cursor: pointer;
}
.header {
  display: flex;
  gap: 1em;
}
.cover {
  width: 90px;
  height: auto;
  flex-shrink: 0;
}
.authors {
  color: #555;
}
.tags {
  color: #888;
  font-size: 0.9em;
}
.custom-columns {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 0.2em 0.75em;
  margin: 0.5em 0 0;
  font-size: 0.9em;
}
.custom-columns dt {
  color: #888;
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
.read {
  background: #2a6df4;
  color: #fff;
  border: none;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.download {
  border: 1px solid #ccc;
  padding: 0.5em 1em;
  border-radius: 4px;
  text-decoration: none;
  color: inherit;
}
.error {
  color: #b00020;
}
.edit {
  background: none;
  border: 1px solid #ccc;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.delete {
  background: none;
  border: 1px solid #d99;
  color: #b00020;
  padding: 0.5em 1em;
  border-radius: 4px;
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
  font-size: 0.85em;
  color: #555;
  gap: 0.2em;
}
.fields input {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
.checkbox-field {
  flex-direction: row !important;
  align-items: center;
  gap: 0.4em !important;
}
.format-manager {
  margin-top: 1em;
  padding-top: 1em;
  border-top: 1px solid #eee;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 0.5em;
}
.format-manager-label {
  font-size: 0.85em;
  color: #555;
  font-weight: 600;
}
.hint {
  font-size: 0.85em;
  color: #888;
}
.template-tester {
  border-top: 1px solid #eee;
  padding-top: 0.75em;
  margin-top: 0.5em;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}
.template-tester textarea {
  font-family: ui-monospace, monospace;
  font-size: 0.85em;
  padding: 0.4em;
  border: 1px solid #ccc;
  border-radius: 4px;
  resize: vertical;
}
.template-tester-actions {
  display: flex;
}
.template-result {
  font-family: ui-monospace, monospace;
  font-size: 0.85em;
  background: #f5f5f5;
  padding: 0.4em 0.6em;
  border-radius: 4px;
  word-break: break-word;
}
.quick-view {
  border-top: 1px solid #eee;
  padding-top: 0.75em;
  margin-top: 0.5em;
  display: flex;
  flex-direction: column;
  gap: 0.75em;
}
.quick-view-group h4 {
  margin: 0 0 0.3em;
  font-size: 0.85em;
  color: #555;
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
  border: 1px solid #ccc;
  border-radius: 4px;
  padding: 0.3em 0.6em;
  font: inherit;
  font-size: 0.85em;
  cursor: pointer;
}
.format-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.3em;
  background: #f0f0f0;
  border-radius: 4px;
  padding: 0.3em 0.5em;
  font-size: 0.85em;
}
.format-chip-remove {
  border: none;
  background: none;
  cursor: pointer;
  color: #888;
  font-size: 0.9em;
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
  background: #f7f7f7;
  border-radius: 6px;
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
  font-size: 0.85em;
  color: #555;
  gap: 0.2em;
}
.convert-row select {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid #ccc;
  border-radius: 4px;
}
.convert-done {
  color: #2a7f2a;
  margin: 0.6em 0 0;
}
.convert-options {
  margin-top: 0.6em;
  font-size: 0.85em;
}
.convert-options summary {
  cursor: pointer;
  color: #555;
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
  color: #333;
}
.convert-options-text {
  flex-direction: column !important;
  align-items: flex-start !important;
}
.convert-options-text input[type="text"],
.convert-options-text input[type="number"] {
  font: inherit;
  padding: 0.3em 0.45em;
  border: 1px solid #ccc;
  border-radius: 4px;
  width: 100%;
  max-width: 20em;
}
</style>
