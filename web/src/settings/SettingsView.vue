<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { isTauri, tauriInvoke } from "../tauri";
import { activeRules, COLORING_RULES_PROFILE, DEFAULT_COLORING_RULES, type ColoringRule, type ColoringRulesPrefs } from "../library/coloringRules";
import { fetchPluginCatalog, inspectPlugin, installFromCatalog, installPlugin, listPlugins, removePlugin, setPluginEnabled, type CatalogPlugin, type InstalledPlugin, getEmailAccount, saveEmailAccount, type EmailAccount } from "../library/api";
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
// Plugin catalog (#1.14). A directory of installable packages --
// a repo folder or git submodule, since plugins for this port have to
// be written against its WASM ABI rather than carried over from
// calibre's Python ones.
const catalog = ref<CatalogPlugin[]>([]);
const catalogConfigured = ref(false);
const catalogBusy = ref(false);
const catalogError = ref<string | null>(null);

async function loadCatalog() {
  catalogBusy.value = true;
  catalogError.value = null;
  try {
    const result = await fetchPluginCatalog();
    catalogConfigured.value = result.configured;
    catalog.value = result.plugins;
  } catch (e) {
    catalogError.value = e instanceof Error ? e.message : String(e);
  } finally {
    catalogBusy.value = false;
  }
}

async function installCatalogPlugin(name: string) {
  catalogBusy.value = true;
  catalogError.value = null;
  try {
    await installFromCatalog(name);
    await Promise.all([loadCatalog(), loadPlugins()]);
  } catch (e) {
    catalogError.value = e instanceof Error ? e.message : String(e);
  } finally {
    catalogBusy.value = false;
  }
}

// Auto-add folder (#4.4). Desktop-only: watching a folder needs real
// filesystem access, which a browser tab has none of.
const autoAddFolder = ref<string | null>(null);
const autoAddBusy = ref(false);
const autoAddError = ref<string | null>(null);

async function loadAutoAddFolder() {
  if (!isTauri()) return;
  try {
    autoAddFolder.value = await tauriInvoke<string | null>("get_auto_add_folder");
  } catch (e) {
    console.error("failed to read the auto-add folder", e);
  }
}

async function chooseAutoAddFolder(clear: boolean) {
  autoAddBusy.value = true;
  autoAddError.value = null;
  try {
    autoAddFolder.value = await tauriInvoke<string | null>("choose_auto_add_folder", { clear });
  } catch (e) {
    autoAddError.value = e instanceof Error ? e.message : String(e);
  } finally {
    autoAddBusy.value = false;
  }
}

// Row colouring rules (#4.1). A rule is a template evaluated per
// book; a usable colour in the result colours that row. First enabled
// matching rule wins, so this list's order is the precedence.
const coloringRules = ref<ColoringRule[]>([]);
const coloringSaved = ref(false);
const coloringError = ref<string | null>(null);

async function loadColoringRules() {
  try {
    const prefs = await fetchProfile<ColoringRulesPrefs>(COLORING_RULES_PROFILE);
    coloringRules.value = prefs?.rules ?? DEFAULT_COLORING_RULES.rules;
  } catch (e) {
    console.error("failed to load colouring rules", e);
  }
}

function addColoringRule() {
  coloringRules.value = [...coloringRules.value, { name: `Rule ${coloringRules.value.length + 1}`, template: "", enabled: true }];
}

function removeColoringRule(index: number) {
  coloringRules.value = coloringRules.value.filter((_, i) => i !== index);
}

function moveColoringRule(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= coloringRules.value.length) return;
  const next = [...coloringRules.value];
  [next[index], next[to]] = [next[to], next[index]];
  coloringRules.value = next;
}

async function saveColoringRules() {
  coloringError.value = null;
  coloringSaved.value = false;
  try {
    await saveProfile(COLORING_RULES_PROFILE, { rules: coloringRules.value });
    coloringSaved.value = true;
  } catch (e) {
    coloringError.value = e instanceof Error ? e.message : String(e);
  }
}

// Email account (#4.2). Everything but the password is persisted --
// see the route's own doc for why the credential is deliberately not.
const emailAccount = ref<EmailAccount>({ relay: "", port: 587, username: "", encryption: "tls", from: "" });
const emailBusy = ref(false);
const emailError = ref<string | null>(null);
const emailSaved = ref(false);

async function loadEmailAccount() {
  try {
    const account = await getEmailAccount();
    if (account) emailAccount.value = { port: 587, encryption: "tls", ...account };
  } catch (e) {
    console.error("failed to load the email account", e);
  }
}

async function persistEmailAccount() {
  emailBusy.value = true;
  emailError.value = null;
  emailSaved.value = false;
  try {
    await saveEmailAccount(emailAccount.value);
    emailSaved.value = true;
  } catch (e) {
    emailError.value = e instanceof Error ? e.message : String(e);
  } finally {
    emailBusy.value = false;
  }
}

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
  void loadCatalog();
  void loadEmailAccount();
  void loadColoringRules();
  void loadAutoAddFolder();
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
        <h3>Auto-add folder</h3>
        <template v-if="isTauri()">
          <p class="hint">
            Books dropped into this folder are added to the library automatically and
            then removed from it. A file that fails to add, or is already in the
            library, is left where it is rather than disappearing.
          </p>
          <p class="field">{{ autoAddFolder || "No folder is being watched." }}</p>
          <div class="plugin-actions">
            <button type="button" :disabled="autoAddBusy" @click="chooseAutoAddFolder(false)">
              {{ autoAddBusy ? "Working…" : autoAddFolder ? "Change folder…" : "Choose folder…" }}
            </button>
            <button v-if="autoAddFolder" type="button" :disabled="autoAddBusy" @click="chooseAutoAddFolder(true)">Stop watching</button>
          </div>
          <p v-if="autoAddError" class="error">{{ autoAddError }}</p>
        </template>
        <p v-else class="hint">Watching a folder needs filesystem access, so it is only available in the desktop app.</p>
      </section>

      <section class="pane">
        <h3>Row colours</h3>
        <p class="hint">
          Each rule is a template evaluated against every book; if it returns a colour
          name or a hex value, that book's row takes it. The first enabled rule that
          matches wins, so order matters — anything else is ignored.
        </p>
        <ul class="coloring-list">
          <li v-for="(r, i) in coloringRules" :key="i" class="coloring-row">
            <label class="field checkbox"><input type="checkbox" v-model="r.enabled" /></label>
            <input v-model="r.name" class="coloring-name" placeholder="Name" />
            <input v-model="r.template" class="coloring-template" placeholder="program: test(field('series'), 'blue', '')" />
            <button type="button" :disabled="i === 0" title="Move up" @click="moveColoringRule(i, -1)">↑</button>
            <button type="button" :disabled="i === coloringRules.length - 1" title="Move down" @click="moveColoringRule(i, 1)">↓</button>
            <button type="button" title="Remove" @click="removeColoringRule(i)">✕</button>
          </li>
        </ul>
        <div class="plugin-actions">
          <button type="button" @click="addColoringRule">+ Add rule</button>
          <button type="button" @click="saveColoringRules">Save colour rules</button>
        </div>
        <p v-if="coloringError" class="error">{{ coloringError }}</p>
        <p v-else-if="coloringSaved" class="status">Saved. {{ activeRules(coloringRules).length }} rule(s) active.</p>
      </section>

      <section class="pane">
        <h3>Email</h3>
        <p class="hint">
          Used when sending a book by email. Everything here is saved except the
          password — this is stored on the server as plain JSON, and the server can
          be reached over a network, so a saved password would be readable by more
          people than you would expect. You will be asked for it when you send.
        </p>
        <label class="field">SMTP server <input v-model="emailAccount.relay" placeholder="smtp.example.com" :disabled="emailBusy" /></label>
        <label class="field">Port <input v-model.number="emailAccount.port" type="number" min="1" max="65535" :disabled="emailBusy" /></label>
        <label class="field">Username <input v-model="emailAccount.username" :disabled="emailBusy" /></label>
        <label class="field">
          Encryption
          <select v-model="emailAccount.encryption" :disabled="emailBusy">
            <option value="tls">STARTTLS</option>
            <option value="ssl">SSL/TLS</option>
            <option value="none">None</option>
          </select>
        </label>
        <label class="field">Send from <input v-model="emailAccount.from" type="email" placeholder="you@example.com" :disabled="emailBusy" /></label>
        <button type="button" :disabled="emailBusy || !emailAccount.relay.trim()" @click="persistEmailAccount">
          {{ emailBusy ? "Saving…" : "Save email settings" }}
        </button>
        <p v-if="emailError" class="error">{{ emailError }}</p>
        <p v-else-if="emailSaved" class="status">Saved.</p>
      </section>

      <section class="pane">
        <h3>Plugins</h3>
        <p class="hint">
          Plugins run in a WebAssembly sandbox. They have no access to your files or the
          network unless they ask for it, and anything they ask for is shown below before
          you install.
        </p>

        <div class="plugin-catalog">
          <h4>Available plugins</h4>
          <p v-if="!catalogConfigured" class="hint">
            No plugin catalog is configured. Start the server with
            <code>--plugin-catalog-dir &lt;path&gt;</code> pointing at a folder of plugin
            packages to browse and install from here.
          </p>
          <template v-else>
            <p v-if="catalog.length === 0" class="hint">The catalog is configured but empty.</p>
            <ul v-else class="catalog-list">
              <li v-for="p in catalog" :key="p.name" class="catalog-row">
                <span class="catalog-name">{{ p.name }}</span>
                <span class="catalog-version">
                  {{ p.version }}
                  <template v-if="p.installed_version && p.installed_version !== p.version">(installed {{ p.installed_version }})</template>
                </span>
                <span class="catalog-desc">{{ p.description }}</span>
                <button v-if="p.update_available" type="button" :disabled="catalogBusy" @click="installCatalogPlugin(p.name)">Update</button>
                <button v-else-if="!p.installed" type="button" :disabled="catalogBusy" @click="installCatalogPlugin(p.name)">Install</button>
                <span v-else class="catalog-installed">Installed</span>
              </li>
            </ul>
          </template>
          <p v-if="catalogError" class="error">{{ catalogError }}</p>
          <button type="button" :disabled="catalogBusy" @click="loadCatalog">{{ catalogBusy ? "Loading…" : "Refresh catalog" }}</button>
        </div>

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
  border: 1px solid var(--border);
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
  border: 1px solid var(--border);
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
  font-size: var(--fs-small);
  color: var(--fg-muted);
  font-weight: normal;
}
.plugin-safe {
  color: var(--success);
  font-size: var(--fs-small);
}
.plugin-warn {
  color: var(--warning);
  font-size: var(--fs-small);
}
.plugin-grants ul {
  margin: 0.25em 0 0.5em 1.2em;
  padding: 0;
  font-size: var(--fs-body);
}
.delete {
  color: var(--danger);
}
.error {
  color: var(--danger);
}
.saved {
  color: var(--success);
}
/* Plugin catalog (#1.14). */
.plugin-catalog {
  margin-bottom: 1rem;
}
.plugin-catalog h4 {
  margin: 0 0 0.3rem;
  font-size: var(--fs-small);
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.6;
}
.catalog-list {
  list-style: none;
  margin: 0 0 0.5rem;
  padding: 0;
}
.catalog-row {
  display: grid;
  grid-template-columns: minmax(10ch, 1fr) auto minmax(0, 1.6fr) auto;
  gap: 0.6rem;
  align-items: baseline;
  padding: 0.25rem 0;
  border-bottom: 1px solid var(--border);
  font-size: var(--fs-small);
}
.catalog-name {
  font-weight: 600;
}
.catalog-version {
  font-variant-numeric: tabular-nums;
  opacity: 0.7;
  white-space: nowrap;
}
.catalog-desc {
  opacity: 0.75;
  overflow: hidden;
  text-overflow: ellipsis;
}
.catalog-installed {
  opacity: 0.55;
  font-size: var(--fs-small);
}

/* Colouring rules (#4.1). */
.coloring-list {
  list-style: none;
  margin: 0 0 0.5rem;
  padding: 0;
}
.coloring-row {
  display: flex;
  gap: 0.35rem;
  align-items: center;
  margin-bottom: 0.3rem;
}
.coloring-name {
  width: 10rem;
}
.coloring-template {
  flex: 1;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: var(--fs-small);
}

</style>
