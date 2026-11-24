<script setup lang="ts">
// In-app help.
//
// The shortcut list is generated from the action registry and the live keymap rather
// than written by hand, so it cannot drift: an action added to library/actions.ts appears
// here automatically, and a rebound key shows its real binding. Above it is a short "how to"
// for the core workflow (import, find duplicates, organize, export) whose buttons open the
// dialogs it talks about (library/help.ts).

import Dialog from "./Dialog.vue";
import { computed, ref } from "vue";

import { type ActionGroup, type LibraryAction, type LibraryActionId } from "../library/actions";
import { HELP_STEPS, helpActions, OTHER_KEYS } from "../library/help";
import { isTauri } from "../tauri";
import { KEYMAP_ACTION_LABELS, type KeymapAction, type KeymapPrefs } from "../settings/api";
import { formatKey } from "../library/shortcuts";

const props = defineProps<{ keymap: KeymapPrefs }>();
const emit = defineEmits<{ close: []; run: [id: LibraryActionId] }>();

const GROUP_TITLES: Record<ActionGroup, string> = {
  library: "Library",
  selection: "Selection",
  book: "Book",
  view: "View",
};

const GROUP_ORDER: ActionGroup[] = ["library", "selection", "book", "view"];

const REQUIREMENT_NOTES: Record<LibraryAction["requires"], string> = {
  none: "",
  selection: "needs at least one book selected",
  "single-selection": "needs exactly one book selected",
  "multi-selection": "needs two or more books selected",
};

/** The binding for an action, if it has one. */
function shortcutFor(id: LibraryActionId): string | null {
  const binding = (props.keymap as Record<string, string | undefined>)[id];
  return binding ? formatKey(binding) : null;
}

const filter = ref("");

// Desktop-only actions are hidden in a browser tab, exactly as they
// are everywhere else -- documenting something the reader cannot use
// is just confusing.
const groups = computed(() => {
  // A label is listed once in the whole list (Fix titles is registered for the library and for the selection).
  const seen = new Set<string>();
  return GROUP_ORDER.map((group) => ({
    group,
    title: GROUP_TITLES[group],
    actions: helpActions(group, { desktop: isTauri(), filter: filter.value }).filter((a) => !seen.has(a.label) && !!seen.add(a.label)),
  })).filter((g) => g.actions.length > 0);
});

/** Reader shortcuts live outside the action registry. */
const readerShortcuts = computed(() =>
  (["readerNext", "readerPrev"] as KeymapAction[]).map((action) => ({
    label: KEYMAP_ACTION_LABELS[action],
    key: props.keymap[action],
  })),
);

function run(id: LibraryActionId) {
  emit("run", id);
}
</script>

<template>
  <Dialog title="Help" size="lg" @close="emit('close')">
    <section class="help-group" aria-labelledby="help-howto">
      <h4 id="help-howto">Getting a collection in order</h4>
      <ol class="help-steps">
        <li v-for="step in HELP_STEPS" :key="step.title">
          <strong>{{ step.title }}.</strong>
          {{ step.text }}
          <span class="help-run">
            <button v-for="r in step.run" :key="r.id" type="button" :data-help-run="r.id" @click="run(r.id)">{{ r.label }}</button>
          </span>
        </li>
      </ol>
    </section>

    <p class="hint">Every action this app knows about, with its current keyboard shortcut. A plain letter is the key on its own (<kbd>d</kbd>); a capital needs Shift (<kbd>Shift+E</kbd>). Shortcuts are rebindable in Settings.</p>
    <input v-model="filter" type="search" class="help-filter" aria-label="Find an action" placeholder="Find an action…" />

    <section v-for="g in groups" :key="g.group" class="help-group">
      <h4>{{ g.title }}</h4>
      <ul class="help-list">
        <li v-for="a in g.actions" :key="a.id">
          <span class="help-label">{{ a.label }}</span>
          <kbd v-if="shortcutFor(a.id)">{{ shortcutFor(a.id) }}</kbd>
          <span v-else class="help-nokey">—</span>
          <span class="help-note">{{ REQUIREMENT_NOTES[a.requires] }}</span>
        </li>
      </ul>
    </section>
    <p v-if="groups.length === 0" class="hint">No action matches “{{ filter }}”.</p>

    <section v-if="!filter" class="help-group">
      <h4>Reader</h4>
      <ul class="help-list">
        <li v-for="r in readerShortcuts" :key="r.label">
          <span class="help-label">{{ r.label }}</span>
          <kbd>{{ formatKey(r.key) }}</kbd>
          <span class="help-note"></span>
        </li>
      </ul>
    </section>

    <section v-if="!filter" class="help-group">
      <h4>Other keys</h4>
      <ul class="help-list">
        <li v-for="k in OTHER_KEYS" :key="k.keys">
          <kbd>{{ k.keys }}</kbd>
          <span class="help-label help-span">{{ k.does }}</span>
        </li>
      </ul>
    </section>

    <p class="hint">Right-click a book for its own actions (with two or more selected: Merge, Compare, Bulk edit). Shortcuts never fire while you are typing.</p>
    <template #footer>
      <button type="button" class="primary" @click="emit('close')">Close</button>
    </template>
  </Dialog>
</template>

<style scoped>
.help-group {
  margin-bottom: 0.9rem;
}
.help-group h4 {
  margin: 0 0 0.3rem;
  font-size: var(--fs-small);
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.6;
}
.help-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.help-steps {
  margin: 0;
  padding-left: 1.4em;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}
.help-run {
  display: inline-flex;
  gap: 0.4em;
  margin-left: 0.4em;
  vertical-align: middle;
  flex-wrap: wrap;
}
.help-filter {
  align-self: flex-start;
  width: min(100%, 22em);
}
.help-span {
  grid-column: 2 / 4;
}
.help-list li {
  display: grid;
  grid-template-columns: minmax(12ch, 1fr) 7rem minmax(0, 1.3fr);
  gap: 0.6rem;
  align-items: baseline;
  padding: 0.15rem 0;
  font-size: var(--fs-small);
}
.help-nokey {
  opacity: 0.35;
}
.help-note {
  opacity: 0.6;
  font-size: var(--fs-small);
}
kbd {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: var(--fs-small);
  border: 1px solid var(--border);
  border-bottom-width: 2px;
  border-radius: var(--radius);
  padding: 0.05em 0.4em;
  justify-self: start;
}
</style>
