<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { commitTweakSession, discardTweakSession, fetchToc, fetchTweakFile, openTweakSession, saveToc, saveTweakFile, type TocNode } from "../library/tweak";
import TocTreeNode from "./TocTreeNode.vue";

import { bookReport, checkBook, fixBookChecks, type BookReport, type CheckResult, spellCheckBook, type MisspelledWord } from "../library/api";

const props = defineProps<{ bookId: number }>();
const emit = defineEmits<{ close: []; updated: [] }>();

const loading = ref(true);
const error = ref<string | null>(null);
const sessionId = ref<string | null>(null);
const files = ref<string[]>([]);
const selectedFile = ref<string | null>(null);
const content = ref("");
const savedContent = ref(""); // last-saved-or-loaded content, to detect unsaved edits
const fileError = ref<string | null>(null);
const saving = ref(false);
const committing = ref(false);

// Real visual TOC tree editor (#760) -- a sibling view onto the same
// open session, alongside the existing plain-text file list.
const showToc = ref(false);
const tocTree = ref<TocNode[]>([]);
const tocLoading = ref(false);
const tocError = ref<string | null>(null);
const tocSaving = ref(false);
const tocSaved = ref(false);

async function openToc() {
  if (!sessionId.value) return;
  showToc.value = true;
  tocLoading.value = true;
  tocError.value = null;
  tocSaved.value = false;
  try {
    tocTree.value = await fetchToc(sessionId.value);
  } catch (e) {
    tocError.value = e instanceof Error ? e.message : String(e);
  } finally {
    tocLoading.value = false;
  }
}

async function saveTocTree() {
  if (!sessionId.value) return;
  tocSaving.value = true;
  tocError.value = null;
  tocSaved.value = false;
  try {
    await saveToc(sessionId.value, tocTree.value);
    tocSaved.value = true;
  } catch (e) {
    tocError.value = e instanceof Error ? e.message : String(e);
  } finally {
    tocSaving.value = false;
  }
}

const dirty = () => content.value !== savedContent.value;

async function open() {
  loading.value = true;
  error.value = null;
  try {
    const session = await openTweakSession(props.bookId);
    sessionId.value = session.session_id;
    files.value = session.files;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
void open();

async function selectFile(name: string) {
  if (!sessionId.value) return;
  if (dirty() && !confirm(`Discard unsaved changes to "${selectedFile.value}"?`)) return;
  fileError.value = null;
  try {
    const text = await fetchTweakFile(sessionId.value, name);
    selectedFile.value = name;
    content.value = text;
    savedContent.value = text;
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  }
}

async function save() {
  if (!sessionId.value || !selectedFile.value) return;
  saving.value = true;
  fileError.value = null;
  try {
    await saveTweakFile(sessionId.value, selectedFile.value, content.value);
    savedContent.value = content.value;
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

async function commitAndClose() {
  if (!sessionId.value) return;
  if (dirty() && !confirm("You have unsaved changes to the current file. Save them before committing?")) return;
  committing.value = true;
  fileError.value = null;
  try {
    if (dirty()) await save();
    await commitTweakSession(sessionId.value);
    sessionId.value = null;
    emit("updated");
    emit("close");
  } catch (e) {
    fileError.value = e instanceof Error ? e.message : String(e);
  } finally {
    committing.value = false;
  }
}

function discardAndClose() {
  if (sessionId.value) void discardTweakSession(sessionId.value);
  sessionId.value = null;
  emit("close");
}

// A real cleanup best-effort, not a guarantee: this port has no
// idle-session reaper (see tweak.rs's own doc) -- closing via the
// browser tab/reload rather than this component's own buttons still
// leaks a session server-side, same disclosed narrowing as
// render_jobs/conversion_jobs.
onBeforeUnmount(() => {
  if (sessionId.value) void discardTweakSession(sessionId.value);
});

// Check book and reports (#3.3 / #3.2). Both engines were fully
// ported with no caller anywhere. They act on the open session, so
// they see unsaved edits -- checking the stored copy would report
// problems already fixed in the editor.
type EditorTab = "files" | "toc" | "check" | "report" | "spell";
const tab = ref<EditorTab>("files");

const checkResult = ref<CheckResult | null>(null);
const checkBusy = ref(false);
const checkError = ref<string | null>(null);
const checkMessage = ref<string | null>(null);

const report = ref<BookReport | null>(null);
const reportBusy = ref(false);
const reportError = ref<string | null>(null);

async function runCheck() {
  if (!sessionId.value) return;
  checkBusy.value = true;
  checkError.value = null;
  checkMessage.value = null;
  try {
    checkResult.value = await checkBook(sessionId.value);
  } catch (e) {
    checkError.value = e instanceof Error ? e.message : String(e);
  } finally {
    checkBusy.value = false;
  }
}

async function runFixes() {
  if (!sessionId.value) return;
  checkBusy.value = true;
  checkError.value = null;
  try {
    const result = await fixBookChecks(sessionId.value);
    checkMessage.value = result.changed ? `Fixed ${result.attempted} problem(s). Commit to keep the changes.` : "Nothing could be fixed automatically.";
    await runCheck();
  } catch (e) {
    checkError.value = e instanceof Error ? e.message : String(e);
  } finally {
    checkBusy.value = false;
  }
}

async function loadReport() {
  if (!sessionId.value) return;
  reportBusy.value = true;
  reportError.value = null;
  try {
    report.value = await bookReport(sessionId.value);
  } catch (e) {
    reportError.value = e instanceof Error ? e.message : String(e);
  } finally {
    reportBusy.value = false;
  }
}

function openTab(next: EditorTab) {
  tab.value = next;
  showToc.value = next === "toc";
  if (next === "toc") void openToc();
  // Both are computed from the live session, so they are re-run on
  // each visit rather than cached -- an edit since last time would
  // make a cached result quietly wrong.
  if (next === "check") void runCheck();
  if (next === "report") void loadReport();
  if (next === "spell") void runSpellCheck();
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

// Spell check (#3.1). Dictionaries ship with the binary (#865), so
// this needs no configuration -- the engine was always real, the data
// was what was missing.
const spellWords = ref<MisspelledWord[]>([]);
const spellBusy = ref(false);
const spellError = ref<string | null>(null);

async function runSpellCheck() {
  if (!sessionId.value) return;
  spellBusy.value = true;
  spellError.value = null;
  try {
    spellWords.value = (await spellCheckBook(sessionId.value)).words;
  } catch (e) {
    spellError.value = e instanceof Error ? e.message : String(e);
  } finally {
    spellBusy.value = false;
  }
}
</script>

<template>
  <div class="backdrop">
    <div class="panel">
      <button class="close" @click="discardAndClose">✕</button>
      <h3>Tweak Book</h3>
      <p v-if="loading">Loading…</p>
      <p v-else-if="error" class="error">{{ error }}</p>
      <template v-else>
        <div class="mode-tabs">
          <button type="button" :class="{ active: tab === 'files' }" @click="openTab('files')">Files</button>
          <button type="button" :class="{ active: tab === 'toc' }" @click="openTab('toc')">Table of contents</button>
          <button type="button" :class="{ active: tab === 'check' }" @click="openTab('check')">Check book</button>
          <button type="button" :class="{ active: tab === 'report' }" @click="openTab('report')">Report</button>
          <button type="button" :class="{ active: tab === 'spell' }" @click="openTab('spell')">Spelling</button>
        </div>

        <section v-if="tab === 'spell'" class="editor-tool">
          <div class="tool-actions">
            <button type="button" :disabled="spellBusy" @click="runSpellCheck">{{ spellBusy ? "Checking…" : "Re-check" }}</button>
          </div>
          <p class="hint">Checked against the en-US dictionary shipped with the app.</p>
          <p v-if="spellError" class="error">{{ spellError }}</p>
          <p v-else-if="!spellBusy && spellWords.length === 0" class="status">No misspellings found.</p>
          <p v-else-if="spellWords.length" class="status">{{ spellWords.length }} word(s) not recognised</p>
          <ul v-if="spellWords.length" class="check-list">
            <li v-for="w in spellWords" :key="w.word">
              <span class="check-level">{{ w.count }}×</span>
              <span class="check-where">{{ w.files.join(", ") }}</span>
              <span class="check-msg"><strong>{{ w.word }}</strong><template v-if="w.suggestions.length"> — {{ w.suggestions.join(", ") }}</template></span>
              <span></span>
            </li>
          </ul>
        </section>

        <section v-else-if="tab === 'check'" class="editor-tool">
          <div class="tool-actions">
            <button type="button" :disabled="checkBusy" @click="runCheck">{{ checkBusy ? "Checking…" : "Re-check" }}</button>
            <button type="button" :disabled="checkBusy || !checkResult?.fixable" @click="runFixes">
              Fix {{ checkResult?.fixable ?? 0 }} automatically
            </button>
          </div>
          <p v-if="checkError" class="error">{{ checkError }}</p>
          <p v-if="checkMessage" class="status">{{ checkMessage }}</p>
          <p v-if="checkResult && checkResult.count === 0" class="status">No problems found.</p>
          <p v-else-if="checkResult" class="status">{{ checkResult.count }} item(s), {{ checkResult.errors }} error(s)</p>
          <ul v-if="checkResult?.items.length" class="check-list">
            <li v-for="(item, i) in checkResult.items" :key="i" :class="`level-${item.level}`">
              <span class="check-level">{{ item.level }}</span>
              <span class="check-where">{{ item.file }}<template v-if="item.line">:{{ item.line }}</template></span>
              <span class="check-msg" :title="item.help">{{ item.message }}</span>
              <span v-if="item.fixable" class="check-fixable">auto-fixable</span>
            </li>
          </ul>
        </section>

        <section v-else-if="tab === 'report'" class="editor-tool">
          <div class="tool-actions">
            <button type="button" :disabled="reportBusy" @click="loadReport">{{ reportBusy ? "Loading…" : "Refresh" }}</button>
          </div>
          <p v-if="reportError" class="error">{{ reportError }}</p>
          <template v-else-if="report">
            <p class="status">{{ report.files.count }} file(s), {{ formatSize(report.files.total_size) }} total · {{ report.images.count }} image(s)</p>
            <ul class="report-list">
              <li v-for="f in report.files.items" :key="f.name">
                <span class="report-name">{{ f.name }}</span>
                <span class="report-category">{{ f.category }}</span>
                <span class="report-size">{{ formatSize(f.size) }}</span>
              </li>
            </ul>
            <ul v-if="report.images.items.length" class="report-list">
              <li v-for="img in report.images.items" :key="img.name">
                <span class="report-name">{{ img.name }}</span>
                <span class="report-category">{{ img.width }}×{{ img.height }}</span>
                <span class="report-size">{{ formatSize(img.size) }} · used {{ img.usage }}×</span>
              </li>
            </ul>
          </template>
        </section>

        <template v-if="tab === 'files'">
          <p class="hint">Plain-text editing of this EPUB's own internal files. No rich editor or live preview yet -- open the edited book in the reader afterward to check your changes.</p>
          <div class="editor">
            <ul class="file-list">
              <li v-for="name in files" :key="name">
                <button type="button" :class="{ active: name === selectedFile }" @click="selectFile(name)">{{ name }}</button>
              </li>
            </ul>
            <div class="file-content">
              <p v-if="!selectedFile" class="hint">Select a file to edit.</p>
              <textarea v-else v-model="content" spellcheck="false"></textarea>
            </div>
          </div>
          <p v-if="fileError" class="error">{{ fileError }}</p>
        </template>

        <template v-else-if="tab === 'toc'">
          <p class="hint">Add, remove, reorder, and rename real table-of-contents entries. A destination points at a file in this book (e.g. "chapter1.xhtml"); ⚠ marks a destination that no longer exists.</p>
          <p v-if="tocLoading">Loading…</p>
          <template v-else>
            <div class="toc-editor">
              <p v-if="tocTree.length === 0" class="hint">No entries yet.</p>
              <TocTreeNode v-model="tocTree" />
              <button type="button" @click="tocTree.push({ title: 'New Entry', dest: null, frag: null, children: [] })">+ Add top-level entry</button>
            </div>
            <p v-if="tocError" class="error">{{ tocError }}</p>
            <p v-if="tocSaved" class="saved">Saved to this session -- click "Save book" to write it to the library.</p>
          </template>
          <div class="actions">
            <button type="button" :disabled="tocLoading || tocSaving" @click="saveTocTree">{{ tocSaving ? "Saving…" : "Save table of contents" }}</button>
          </div>
        </template>

        <div class="actions">
          <button type="button" :disabled="!selectedFile || saving" @click="save">{{ saving ? "Saving…" : "Save file" }}</button>
          <button type="button" class="read" :disabled="committing" @click="commitAndClose">{{ committing ? "Saving book…" : "Save book" }}</button>
          <button type="button" :disabled="committing" @click="discardAndClose">Discard all changes</button>
        </div>
      </template>
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
  width: 90%;
  max-width: 900px;
  height: 80vh;
  overflow: hidden;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 0.75em;
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
h3 {
  margin: 0;
}
.hint {
  color: #888;
  font-size: 0.85em;
  margin: 0;
}
.editor {
  display: flex;
  gap: 1em;
  flex: 1;
  min-height: 0;
}
.file-list {
  list-style: none;
  margin: 0;
  padding: 0;
  width: 260px;
  flex-shrink: 0;
  overflow: auto;
  border: 1px solid #ddd;
  border-radius: 4px;
}
.file-list button {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  padding: 0.35em 0.5em;
  cursor: pointer;
  font: inherit;
  font-size: 0.85em;
  word-break: break-all;
}
.file-list button.active {
  background: #2a6df4;
  color: #fff;
}
.file-content {
  flex: 1;
  min-width: 0;
  display: flex;
}
.file-content textarea {
  flex: 1;
  font-family: ui-monospace, monospace;
  font-size: 0.85em;
  padding: 0.5em;
  border: 1px solid #ddd;
  border-radius: 4px;
  resize: none;
}
.actions {
  display: flex;
  gap: 0.5em;
}
.read {
  background: #2a6df4;
  color: #fff;
  border: none;
  padding: 0.5em 1em;
  border-radius: 4px;
  cursor: pointer;
}
.error {
  color: #b00020;
}
.saved {
  color: #1b7f3a;
  font-size: 0.85em;
  margin: 0;
}
.mode-tabs {
  display: flex;
  gap: 0.5em;
}
.mode-tabs button {
  padding: 0.35em 0.75em;
  border: 1px solid #ddd;
  border-radius: 4px;
  background: #f7f7f7;
  cursor: pointer;
}
.mode-tabs button.active {
  background: #2a6df4;
  color: #fff;
  border-color: #2a6df4;
}
.toc-editor {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid #ddd;
  border-radius: 4px;
  padding: 0.5em;
}
/* Check book and report panes (#3.3 / #3.2). */
.editor-tool {
  max-height: 55vh;
  overflow-y: auto;
}
.tool-actions {
  display: flex;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
}
.check-list,
.report-list {
  list-style: none;
  margin: 0 0 0.6rem;
  padding: 0;
  font-size: 0.84rem;
}
.check-list li {
  display: grid;
  grid-template-columns: 5rem minmax(8ch, 1fr) minmax(0, 2.2fr) auto;
  gap: 0.5rem;
  align-items: baseline;
  padding: 0.2rem 0;
  border-bottom: 1px solid #eee;
}
.check-level {
  text-transform: uppercase;
  font-size: 0.72rem;
  letter-spacing: 0.05em;
  opacity: 0.7;
}
.level-error .check-level,
.level-critical .check-level {
  color: #b3261e;
  opacity: 1;
  font-weight: 600;
}
.level-warning .check-level {
  color: #9a6b08;
  opacity: 1;
}
.check-where {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.78rem;
  opacity: 0.75;
  overflow: hidden;
  text-overflow: ellipsis;
}
.check-msg {
  overflow: hidden;
  text-overflow: ellipsis;
}
.check-fixable {
  font-size: 0.72rem;
  opacity: 0.6;
  white-space: nowrap;
}
.report-list li {
  display: grid;
  grid-template-columns: minmax(10ch, 2fr) minmax(6ch, 1fr) auto;
  gap: 0.5rem;
  padding: 0.15rem 0;
  border-bottom: 1px solid #f0f0f0;
}
.report-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.78rem;
  overflow: hidden;
  text-overflow: ellipsis;
}
.report-category,
.report-size {
  opacity: 0.7;
  white-space: nowrap;
}
@media (prefers-color-scheme: dark) {
  .check-list li { border-bottom-color: #2b3037; }
  .report-list li { border-bottom-color: #2b3037; }
}

</style>
