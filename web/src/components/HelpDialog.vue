<script setup lang="ts">
// In-app help (issue 1.17 of the #816 epic).
//
// Generated from the action registry and the live keymap rather than
// written by hand, so it cannot drift: an action added to
// library/actions.ts appears here automatically, and a rebound key
// shows its real binding rather than the default it shipped with.
//
// That is the whole point. Hand-written shortcut documentation is
// wrong the first time anyone rebinds a key, and nothing fails when
// it goes stale.

import { computed } from "vue";

import { LIBRARY_ACTIONS, type ActionGroup, type LibraryAction, type LibraryActionId } from "../library/actions";
import { isTauri } from "../tauri";
import { KEYMAP_ACTION_LABELS, type KeymapAction, type KeymapPrefs } from "../settings/api";

const props = defineProps<{ keymap: KeymapPrefs }>();
const emit = defineEmits<{ close: [] }>();

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
};

/** The binding for an action, if it has one. */
function shortcutFor(id: LibraryActionId): string | null {
  const binding = (props.keymap as Record<string, string | undefined>)[id];
  return binding || null;
}

// Desktop-only actions are hidden in a browser tab, exactly as they
// are everywhere else -- documenting something the reader cannot use
// is just confusing.
const groups = computed(() =>
  GROUP_ORDER.map((group) => ({
    group,
    title: GROUP_TITLES[group],
    actions: LIBRARY_ACTIONS.filter((a) => a.group === group && (!a.desktopOnly || isTauri())),
  })).filter((g) => g.actions.length > 0),
);

/** Reader shortcuts live outside the action registry. */
const readerShortcuts = computed(() =>
  (["readerNext", "readerPrev"] as KeymapAction[]).map((action) => ({
    label: KEYMAP_ACTION_LABELS[action],
    key: props.keymap[action],
  })),
);
</script>

<template>
  <div class="manage-backdrop" @click.self="emit('close')">
    <div class="manage-panel help-panel">
      <h3>Help</h3>
      <p class="hint">Every action this app knows about, with its current keyboard shortcut. Shortcuts are rebindable in Settings.</p>

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

      <section class="help-group">
        <h4>Reader</h4>
        <ul class="help-list">
          <li v-for="r in readerShortcuts" :key="r.label">
            <span class="help-label">{{ r.label }}</span>
            <kbd>{{ r.key }}</kbd>
            <span class="help-note"></span>
          </li>
        </ul>
      </section>

      <p class="hint">Right-click a book for its own actions. Shortcuts never fire while you are typing.</p>
      <button type="button" @click="emit('close')">Close</button>
    </div>
  </div>
</template>

<style scoped>
.help-panel {
  min-width: min(640px, 92vw);
  max-height: 84vh;
  overflow-y: auto;
}
.help-group {
  margin-bottom: 0.9rem;
}
.help-group h4 {
  margin: 0 0 0.3rem;
  font-size: 0.78rem;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.6;
}
.help-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.help-list li {
  display: grid;
  grid-template-columns: minmax(12ch, 1fr) 7rem minmax(0, 1.3fr);
  gap: 0.6rem;
  align-items: baseline;
  padding: 0.15rem 0;
  font-size: 0.87rem;
}
.help-nokey {
  opacity: 0.35;
}
.help-note {
  opacity: 0.6;
  font-size: 0.85em;
}
kbd {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8em;
  border: 1px solid #ccc;
  border-bottom-width: 2px;
  border-radius: 3px;
  padding: 0.05em 0.4em;
  justify-self: start;
}
@media (prefers-color-scheme: dark) {
  kbd {
    border-color: #4a4d55;
  }
}
</style>
