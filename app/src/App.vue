<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

// The real library UI is the browser app served from web/dist, spawned
// and navigated to by the Rust side (app/src-tauri/src/lib.rs) once a
// library is chosen or a previously-persisted one reopens. This view
// only ever shows during that brief window, or if no library has been
// chosen yet.
//
// That second case is the first thing a new user sees, and it used to
// be one sentence and one button: "Choose a Calibre library folder to
// get started". It offered no way to *create* a library, no recents,
// and no hint that an empty folder is all a library needs -- which is
// exactly the confusion it exists to prevent.

const status = ref<"checking" | "no-library" | "opening" | "error">("checking");
const errorMessage = ref("");
const recents = ref<string[]>([]);

/** Shown when "Create a library" is chosen: the name comes first. */
const creating = ref(false);
const newName = ref("");

async function checkPersistedLibrary() {
  const [path, recentList] = await Promise.all([
    invoke<string | null>("get_persisted_library"),
    // Not fatal if this fails -- the recents list is a convenience,
    // and the two real buttons work without it.
    invoke<string[]>("list_recent_libraries").catch(() => [] as string[]),
  ]);
  recents.value = recentList;
  status.value = path ? "opening" : "no-library";
}

function fail(e: unknown) {
  status.value = "error";
  errorMessage.value = e instanceof Error ? e.message : String(e);
}

async function pickLibrary() {
  status.value = "opening";
  errorMessage.value = "";
  try {
    const opened = await invoke<boolean>("choose_library");
    // On success the Rust side navigates the window itself; nothing
    // left to do here. A cancelled dialog is not an error.
    if (!opened) status.value = "no-library";
  } catch (e) {
    fail(e);
  }
}

async function createLibrary() {
  const name = newName.value.trim();
  if (!name) return;
  errorMessage.value = "";
  try {
    const created = await invoke<string | null>("create_library", { name });
    if (created) status.value = "opening";
    // `null` means the folder dialog was cancelled -- stay put rather
    // than discarding the name that was just typed.
  } catch (e) {
    // Recoverable (the name may just be taken), so keep the form open
    // with the message rather than dropping to the error screen.
    errorMessage.value = e instanceof Error ? e.message : String(e);
  }
}

async function openRecent(path: string) {
  status.value = "opening";
  errorMessage.value = "";
  try {
    await invoke("open_recent_library", { path });
  } catch (e) {
    fail(e);
  }
}

/** The folder name, which is what a user calls a library. */
function shortName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

onMounted(checkPersistedLibrary);
</script>

<template>
  <main class="splash">
    <div v-if="status === 'checking' || status === 'opening'" class="waiting">
      <h1>calibre-oxide</h1>
      <p class="hint">{{ status === "checking" ? "Looking for your library…" : "Starting your library…" }}</p>
    </div>

    <div v-else class="card">
      <h1>calibre-oxide</h1>

      <p v-if="status === 'error'" class="error">Could not open that library: {{ errorMessage }}</p>

      <!--
        Says what a library *is*. The single most common confusion:
        people look for a file to open, because that is what most
        applications mean by "open".
      -->
      <p class="lede">A library is a folder on disk holding your books and their metadata.</p>

      <template v-if="!creating">
        <div class="actions">
          <button type="button" class="primary" @click="creating = true">Create a library…</button>
          <button type="button" @click="pickLibrary">Open a library…</button>
        </div>
        <p class="sub">Creating one makes an empty folder ready for books. Opening one can point at an existing folder, or at an empty folder to start fresh.</p>
      </template>

      <form v-else class="create" @submit.prevent="createLibrary">
        <label for="lib-name">Name the library</label>
        <input id="lib-name" v-model="newName" placeholder="e.g. Fiction" autofocus />
        <p v-if="errorMessage" class="error">{{ errorMessage }}</p>
        <div class="actions">
          <button type="submit" class="primary" :disabled="!newName.trim()">Choose where to put it…</button>
          <button type="button" @click="creating = false; errorMessage = ''">Back</button>
        </div>
      </form>

      <section v-if="recents.length && !creating" class="recents">
        <h2>Recent</h2>
        <ul>
          <li v-for="path in recents" :key="path">
            <button type="button" class="recent" :title="path" @click="openRecent(path)">
              <span class="recent-name">{{ shortName(path) }}</span>
              <span class="recent-path">{{ path }}</span>
            </button>
          </li>
        </ul>
      </section>
    </div>

    <!--
      Dropping files anywhere on the window already adds them — the
      handler lives in lib.rs. Nothing on screen said so, which made a
      working feature invisible.
    -->
    <p v-if="status === 'no-library' && !creating" class="drop-hint">…or drag a folder of books onto this window</p>
  </main>
</template>

<style scoped>
.splash {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--sp-7);
  min-height: 100vh;
  padding: var(--sp-8);
  text-align: center;
}

h1 {
  margin: 0 0 4px;
  font-size: var(--fs-large);
  font-weight: 600;
  letter-spacing: -0.01em;
}

.waiting .hint,
.hint {
  color: var(--fg-muted);
  margin: 0;
}

.card {
  display: flex;
  flex-direction: column;
  gap: var(--sp-6);
  width: min(420px, 100%);
  padding: var(--sp-7) var(--sp-8);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  background: var(--bg-raised);
}

.lede {
  margin: 0;
  color: var(--fg-muted);
  line-height: 1.5;
}

.sub {
  margin: 0;
  font-size: var(--fs-small);
  color: var(--fg-faint);
  line-height: 1.5;
}

.actions {
  display: flex;
  gap: var(--sp-4);
  justify-content: center;
}
.actions button {
  flex: 1;
}

.create {
  display: flex;
  flex-direction: column;
  gap: var(--sp-4);
  text-align: left;
}
.create label {
  font-size: var(--fs-small);
  color: var(--fg-muted);
}

.recents {
  border-top: 1px solid var(--border);
  padding-top: 12px;
  text-align: left;
}
.recents h2 {
  margin: 0 0 6px;
  font-size: var(--fs-micro);
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--fg-faint);
}
.recents ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
/* Each recent is one control showing both the name you think of it by
   and the path that disambiguates two libraries with the same name. */
.recent {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  width: 100%;
  height: auto;
  padding: 5px 8px;
  border: none;
  background: none;
  text-align: left;
}
.recent:hover {
  background: var(--bg-hover);
}
.recent-name {
  font-weight: 600;
}
.recent-path {
  font-size: var(--fs-micro);
  color: var(--fg-faint);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.drop-hint {
  margin: 0;
  font-size: var(--fs-small);
  color: var(--fg-faint);
}

.error {
  margin: 0;
  color: var(--danger);
  font-size: var(--fs-small);
}
</style>
