<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { addFormat, deleteBooks, fetchBook, fetchConversionBookData, getConversionStatus, removeFormat, setCover, setFields, shareEmail, startConversion } from "../library/api";
import type { SmtpRelayConfig } from "../library/api";
import type { BookFieldChanges, BookSummary } from "../library/types";

const props = defineProps<{ bookId: number }>();
const emit = defineEmits<{ close: []; updated: []; deleted: [bookId: number] }>();
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

async function openConvert() {
  convertOpen.value = true;
  convertDone.value = false;
  convertError.value = null;
  convertLoadingFormats.value = true;
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
    const jobId = await startConversion(props.bookId, convertInputFmt.value, convertOutputFmt.value);
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

function startEditing() {
  const b = book.value;
  if (!b) return;
  editTitle.value = b.title;
  editAuthors.value = (b.authors ?? []).join(" & ");
  editSeries.value = b.series ?? "";
  editSeriesIndex.value = b.series_index != null ? String(b.series_index) : "";
  editTags.value = (b.tags ?? []).join(", ");
  editRating.value = b.rating ?? 0;
  saveError.value = null;
  editing.value = true;
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

// Only formats the reader MVP (#499) actually round-trips through
// render_book are offered a "Read" link -- other formats still get a
// plain download link.
const READABLE_FORMATS = ["epub", "kepub"];

const readableFormat = computed(() => book.value?.formats.find((f) => READABLE_FORMATS.includes(f)) ?? null);

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
        <p v-if="deleteError" class="error">{{ deleteError }}</p>

        <div class="formats">
          <button v-if="readableFormat" class="read" @click="read">Read ({{ readableFormat.toUpperCase() }})</button>
          <a v-for="[fmt, url] in formatLinks" :key="fmt" :href="url" class="download"> Download {{ fmt.toUpperCase() }} </a>
          <button class="edit" @click="startEditing">Edit metadata</button>
          <button class="edit" @click="openConvert">Convert…</button>
          <button class="edit" @click="openShare">Send…</button>
          <button class="delete" :disabled="deleting" @click="deleteBook">{{ deleting ? "Deleting…" : "Delete" }}</button>
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

        <p v-if="saveError" class="error">{{ saveError }}</p>

        <div class="formats">
          <button type="submit" class="read" :disabled="saving">{{ saving ? "Saving…" : "Save" }}</button>
          <button type="button" @click="editing = false" :disabled="saving">Cancel</button>
        </div>
      </form>
    </div>
  </div>
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
</style>
