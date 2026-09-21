<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { isTauri, tauriInvoke } from "../tauri";
import { inspectPlugin, installPlugin, listPlugins, removePlugin, setPluginEnabled, type InstalledPlugin } from "../library/api";
import { DEFAULT_KEYMAP, DEFAULT_LIBRARY_PREFS, DEFAULT_READER_PREFS, DEFAULT_TOOLBAR_PREFS, fetchProfile, KEYMAP_ACTION_LABELS, KEYMAP_PROFILE, LIBRARY_PREFS_PROFILE, READER_PREFS_PROFILE, saveProfile, TOOLBAR_ACTIONS, TOOLBAR_PREFS_PROFILE, type KeymapAction, type KeymapPrefs, type LibraryPrefs, type ReaderPrefs, type ToolbarActionId, type ToolbarPrefs } from "./api";

const libraryPrefs = ref<LibraryPrefs>({ ...DEFAULT_LIBRARY_PREFS });
const readerPrefs = ref<ReaderPrefs>({ ...DEFAULT_READER_PREFS });
const keymap = ref<KeymapPrefs>({ ...DEFAULT_KEYMAP });
const rebindingAction = ref<KeymapAction | null>(null);

// Real toolbar customization (#753) -- `toolbarOrder` always holds
// every real TOOLBAR_ACTIONS id (defaulting to the registry's own
// declared order the first time this loads), so the reorder UI below
// is just "swap two adjacent list entries," not a partial-list merge.
const toolbarHidden = ref<Set<ToolbarActionId>>(new Set());
const toolbarOrder = ref<ToolbarActionId[]>(TOOLBAR_ACTIONS.map((a) => a.id));

// The "(desktop app only)" note used to be baked into two of the
// registry's label strings. It is now derived from the action's own
// `desktopOnly` flag (#817), so the annotation can never disagree with
// the flag that actually controls whether the button renders.
function toolbarLabel(id: ToolbarActionId): string {
  const action = TOOLBAR_ACTIONS.find((a) => a.id === id);
  if (!action) return id;
  return action.desktopOnly ? `${action.label} (desktop app only)` : action.label;
}
function toggleToolbarHidden(id: ToolbarActionId) {
  const next = new Set(toolbarHidden.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  toolbarHidden.value = next;
}
function moveToolbarAction(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= toolbarOrder.value.length) return;
  const arr = [...toolbarOrder.value];
  [arr[index], arr[to]] = [arr[to], arr[index]];
  toolbarOrder.value = arr;
}
async function saveToolbarPrefs() {
  savedMessage.value = null;
  error.value = null;
  try {
    const prefs: ToolbarPrefs = { hidden: [...toolbarHidden.value], order: toolbarOrder.value };
    await saveProfile(TOOLBAR_PREFS_PROFILE, prefs as unknown as Record<string, unknown>);
    savedMessage.value = "Toolbar layout saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
// Plugin management (#801). Plugins run in a WASM sandbox with no
// filesystem and no network unless their own manifest asks for it, so
// the UI's job is to make that request visible -- especially BEFORE
// install, via the inspect step.
const plugins = ref<InstalledPlugin[]>([]);
const pluginsAvailable = ref(false);
const pluginError = ref<string | null>(null);
const pluginPath = ref("");
const pluginPreview = ref<InstalledPlugin | null>(null);
const pluginBusy = ref(false);

async function loadPlugins() {
  pluginError.value = null;
  try {
    plugins.value = (await listPlugins()).plugins;
    pluginsAvailable.value = true;
  } catch {
    // A server started without --plugin-dir answers 503 here. That is
    // a configuration state, not an error to shout about, so the whole
    // pane is simply hidden.
    pluginsAvailable.value = false;
  }
}

async function inspectPluginPath() {
  pluginError.value = null;
  pluginPreview.value = null;
  pluginBusy.value = true;
  try {
    pluginPreview.value = await inspectPlugin(pluginPath.value.trim());
  } catch (e) {
    pluginError.value = e instanceof Error ? e.message : String(e);
  } finally {
    pluginBusy.value = false;
  }
}

async function confirmInstallPlugin() {
  pluginError.value = null;
  pluginBusy.value = true;
  try {
    await installPlugin(pluginPath.value.trim());
    pluginPreview.value = null;
    pluginPath.value = "";
    await loadPlugins();
    savedMessage.value = "Plugin installed.";
  } catch (e) {
    pluginError.value = e instanceof Error ? e.message : String(e);
  } finally {
    pluginBusy.value = false;
  }
}

async function removePluginClick(name: string) {
  pluginError.value = null;
  try {
    await removePlugin(name);
    await loadPlugins();
  } catch (e) {
    pluginError.value = e instanceof Error ? e.message : String(e);
  }
}

async function togglePlugin(p: InstalledPlugin) {
  pluginError.value = null;
  try {
    await setPluginEnabled(p.name, !p.enabled);
    await loadPlugins();
  } catch (e) {
    // Includes the real "this plugin declares it cannot be disabled"
    // refusal -- surfaced rather than silently ignored.
    pluginError.value = e instanceof Error ? e.message : String(e);
  }
}

const loading = ref(true);
const savedMessage = ref<string | null>(null);
const error = ref<string | null>(null);

const autoReopen = ref(true);
const showAppSettings = ref(false);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const [lib, reader, keys, toolbar] = await Promise.all([fetchProfile<LibraryPrefs>(LIBRARY_PREFS_PROFILE), fetchProfile<ReaderPrefs>(READER_PREFS_PROFILE), fetchProfile<KeymapPrefs>(KEYMAP_PROFILE), fetchProfile<ToolbarPrefs>(TOOLBAR_PREFS_PROFILE)]);
    if (lib) libraryPrefs.value = { ...DEFAULT_LIBRARY_PREFS, ...lib };
    if (reader) readerPrefs.value = { ...DEFAULT_READER_PREFS, ...reader };
    if (keys) keymap.value = { ...DEFAULT_KEYMAP, ...keys };
    if (toolbar) {
      toolbarHidden.value = new Set(toolbar.hidden ?? DEFAULT_TOOLBAR_PREFS.hidden);
      // A saved order might predate a newly-added registry action (or
      // simply be empty, the real default) -- append anything missing
      // at the end rather than dropping it from the reorder UI.
      const saved = toolbar.order?.length ? toolbar.order : TOOLBAR_ACTIONS.map((a) => a.id);
      const missing = TOOLBAR_ACTIONS.map((a) => a.id).filter((id) => !saved.includes(id));
      toolbarOrder.value = [...saved, ...missing];
    }
    await loadPlugins();
    if (isTauri()) {
      showAppSettings.value = true;
      autoReopen.value = await tauriInvoke<boolean>("get_auto_reopen");
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(() => {
  load();
  window.addEventListener("keydown", onRebindKeydown);
});
onBeforeUnmount(() => window.removeEventListener("keydown", onRebindKeydown));

async function saveLibraryPrefs() {
  savedMessage.value = null;
  error.value = null;
  try {
    await saveProfile(LIBRARY_PREFS_PROFILE, libraryPrefs.value as unknown as Record<string, unknown>);
    savedMessage.value = "Library preferences saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function saveReaderPrefs() {
  savedMessage.value = null;
  error.value = null;
  try {
    await saveProfile(READER_PREFS_PROFILE, readerPrefs.value as unknown as Record<string, unknown>);
    savedMessage.value = "Reading preferences saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

function startRebind(action: KeymapAction) {
  rebindingAction.value = action;
}

function onRebindKeydown(e: KeyboardEvent) {
  if (!rebindingAction.value) return;
  e.preventDefault();
  if (e.key === "Escape") {
    rebindingAction.value = null;
    return;
  }
  keymap.value = { ...keymap.value, [rebindingAction.value]: e.key };
  rebindingAction.value = null;
}

async function saveKeymap() {
  savedMessage.value = null;
  error.value = null;
  try {
    await saveProfile(KEYMAP_PROFILE, keymap.value as unknown as Record<string, unknown>);
    savedMessage.value = "Keyboard shortcuts saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function toggleAutoReopen() {
  error.value = null;
  try {
    await tauriInvoke<void>("set_auto_reopen", { enabled: autoReopen.value });
    savedMessage.value = "App settings saved.";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div class="settings">
    <header class="toolbar">
      <router-link to="/" class="back">Library</router-link>
      <h2>Settings</h2>
    </header>

    <p v-if="loading" class="status">Loading…</p>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="savedMessage" class="saved">{{ savedMessage }}</p>

    <template v-if="!loading">
      <section class="pane">
        <h3>Library</h3>
        <label class="field">
          Default sort
          <select v-model="libraryPrefs.sort">
            <option value="timestamp">Date added</option>
            <option value="title">Title</option>
            <option value="authors">Author</option>
            <option value="pubdate">Publication date</option>
            <option value="rating">Rating</option>
            <option value="series">Series</option>
          </select>
        </label>
        <label class="field">
          Default sort order
          <select v-model="libraryPrefs.sortOrder">
            <option value="asc">Ascending</option>
            <option value="desc">Descending</option>
          </select>
        </label>
        <label class="field">
          Books per page
          <input v-model.number="libraryPrefs.pageSize" type="number" min="6" max="200" step="1" />
        </label>
        <label class="field">
          When adding a book that looks like a duplicate
          <select v-model="libraryPrefs.duplicateDefault">
            <option value="ask">Ask every time</option>
            <option value="add">Always add it anyway</option>
            <option value="skip">Always skip it</option>
          </select>
        </label>
        <button type="button" @click="saveLibraryPrefs">Save library settings</button>
      </section>

      <section class="pane">
        <h3>Reading</h3>
        <label class="field">
          Font size ({{ readerPrefs.fontSizePercent }}%)
          <input v-model.number="readerPrefs.fontSizePercent" type="range" min="60" max="220" step="10" />
        </label>
        <label class="field">
          Theme
          <select v-model="readerPrefs.theme">
            <option value="light">Light</option>
            <option value="dark">Dark</option>
            <option value="sepia">Sepia</option>
          </select>
        </label>
        <button type="button" @click="saveReaderPrefs">Save reading settings</button>
      </section>

      <section class="pane">
        <h3>Keyboard shortcuts</h3>
        <div v-for="(label, action) in KEYMAP_ACTION_LABELS" :key="action" class="field keymap-row">
          <span>{{ label }}</span>
          <button type="button" @click="startRebind(action as KeymapAction)">
            {{ rebindingAction === action ? "Press a key… (Esc to cancel)" : keymap[action as KeymapAction] }}
          </button>
        </div>
        <button type="button" @click="saveKeymap">Save keyboard shortcuts</button>
      </section>

      <section class="pane">
        <h3>Toolbar</h3>
        <p class="hint">Show/hide and reorder the library toolbar's own action buttons.</p>
        <ul class="toolbar-list">
          <li v-for="(id, i) in toolbarOrder" :key="id" class="toolbar-row">
            <label class="field checkbox">
              <input type="checkbox" :checked="!toolbarHidden.has(id)" @change="toggleToolbarHidden(id)" />
              {{ toolbarLabel(id) }}
            </label>
            <button type="button" :disabled="i === 0" @click="moveToolbarAction(i, -1)" title="Move up">↑</button>
            <button type="button" :disabled="i === toolbarOrder.length - 1" @click="moveToolbarAction(i, 1)" title="Move down">↓</button>
          </li>
        </ul>
        <button type="button" @click="saveToolbarPrefs">Save toolbar layout</button>
      </section>

      <section v-if="pluginsAvailable" class="pane">
        <h3>Plugins</h3>
        <p class="hint">
          Plugins run in a WebAssembly sandbox. They have no access to your files or the
          network unless they ask for it, and anything they ask for is shown below before
          you install.
        </p>

        <label class="field">
          Install from a plugin package (.zip)
          <input v-model="pluginPath" type="text" placeholder="/path/to/plugin.zip" />
        </label>
        <div class="plugin-actions">
          <button type="button" :disabled="pluginBusy || !pluginPath.trim()" @click="inspectPluginPath">
            {{ pluginBusy ? "Reading…" : "Review before installing" }}
          </button>
        </div>

        <div v-if="pluginPreview" class="plugin-preview">
          <h4>{{ pluginPreview.name }} {{ pluginPreview.version }}</h4>
          <p class="plugin-meta">by {{ pluginPreview.author || "unknown" }} — {{ pluginPreview.description }}</p>
          <p v-if="pluginPreview.capabilities.fully_sandboxed" class="plugin-safe">
            ✓ Fully sandboxed — requests no file or network access.
          </p>
          <div v-else class="plugin-grants">
            <p class="plugin-warn">This plugin is asking for access:</p>
            <ul>
              <li v-for="host in pluginPreview.capabilities.allowed_hosts" :key="host">
                Network access to <code>{{ host }}</code>
              </li>
              <li v-for="hostPath in Object.keys(pluginPreview.capabilities.allowed_paths)" :key="hostPath">
                File access to <code>{{ hostPath }}</code>
              </li>
            </ul>
          </div>
          <button type="button" :disabled="pluginBusy" @click="confirmInstallPlugin">Install this plugin</button>
        </div>

        <p v-if="pluginError" class="error">{{ pluginError }}</p>

        <ul v-if="plugins.length" class="plugin-list">
          <li v-for="p in plugins" :key="p.name" class="plugin-row">
            <div class="plugin-body">
              <div class="plugin-name">{{ p.name }} <span class="plugin-version">{{ p.version }}</span></div>
              <div class="plugin-meta">{{ p.plugin_type }} — {{ p.description }}</div>
              <div v-if="p.capabilities.fully_sandboxed" class="plugin-safe">Fully sandboxed</div>
              <div v-else class="plugin-warn">Can reach: {{ p.capabilities.allowed_hosts.join(", ") || "(files)" }}</div>
            </div>
            <button type="button" @click="togglePlugin(p)">{{ p.enabled ? "Disable" : "Enable" }}</button>
            <button type="button" class="delete" @click="removePluginClick(p.name)">Remove</button>
          </li>
        </ul>
        <p v-else class="hint">No plugins installed.</p>
      </section>

      <section v-if="showAppSettings" class="pane">
        <h3>App</h3>
        <label class="field checkbox">
          <input v-model="autoReopen" type="checkbox" @change="toggleAutoReopen" />
          Reopen the last library automatically on launch
        </label>
      </section>
    </template>
  </div>
</template>

<style scoped>
.settings {
  max-width: 640px;
  margin: 0 auto;
  padding: 1em;
  display: flex;
  flex-direction: column;
  gap: 1.25em;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 1em;
}
.toolbar h2 {
  margin: 0;
}
.pane {
  display: flex;
  flex-direction: column;
  gap: 0.75em;
  padding: 1em;
  border: 1px solid #ddd;
  border-radius: 6px;
}
.pane h3 {
  margin: 0;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
}
.field.checkbox {
  flex-direction: row;
  align-items: center;
}
.keymap-row {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
}
.toolbar-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.3em;
}
.toolbar-row {
  display: flex;
  align-items: center;
  gap: 0.5em;
}
.toolbar-row .field.checkbox {
  flex: 1;
}
.plugin-actions {
  display: flex;
  gap: 0.5em;
}
.plugin-preview,
.plugin-row {
  border: 1px solid #ddd;
  border-radius: 4px;
  padding: 0.75em;
}
.plugin-preview h4 {
  margin: 0 0 0.25em;
}
.plugin-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}
.plugin-row {
  display: flex;
  align-items: center;
  gap: 0.5em;
}
.plugin-body {
  flex: 1;
  min-width: 0;
}
.plugin-name {
  font-weight: 600;
}
.plugin-version,
.plugin-meta {
  font-size: 0.85em;
  color: #666;
  font-weight: normal;
}
.plugin-safe {
  color: #1b7f3a;
  font-size: 0.85em;
}
.plugin-warn {
  color: #a05a00;
  font-size: 0.85em;
}
.plugin-grants ul {
  margin: 0.25em 0 0.5em 1.2em;
  padding: 0;
  font-size: 0.9em;
}
.delete {
  color: #b00020;
}
.error {
  color: #b00020;
}
.saved {
  color: #1b7f3a;
}
</style>
