<script setup lang="ts">
// One toolbar slot: icon above label, with an optional dropdown.
//
// calibre's toolbar is legible at seventeen items because each item is
// a *family* -- the body runs the action you reach for most, and the
// chevron holds its relatives. A flat row of thirty buttons is
// unreadable; a flat row of six buries the other twenty-four behind an
// undifferentiated "More".
//
// See docs/UI_DESIGN.md §2.3 for the behaviour this implements.

import { computed, ref } from "vue";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{
  label: string;
  /** App icon name; `src/icons.ts` maps it to a Lucide component. */
  icon?: string;
  /** Renders the chevron and enables the menu affordances. */
  hasMenu?: boolean;
  disabled?: boolean;
  /** Drawn pressed — used for toggles like "select mode". */
  active?: boolean;
  title?: string;
  /** Icon-only, for a narrow window. The label becomes the tooltip. */
  compact?: boolean;
  /** What the arrow is called to a screen reader ("Virtual library options"); defaults to the label. */
  menuLabel?: string;
  /** The menu this button opens is showing right now. */
  expanded?: boolean;
  /** The body itself opens the menu (a menu slot, "More"), so it is the popup trigger too. */
  bodyOpensMenu?: boolean;
}>();

const emit = defineEmits<{
  /**
   * The body was clicked: run the primary action. Carries the button's anchor,
   * for a slot whose body opens a menu: the menu belongs under the button, not
   * at the window's corner.
   */
  run: [anchor: { x: number; y: number }];
  /** The menu should open, anchored to this rectangle. */
  menu: [anchor: { x: number; y: number }];
}>();

const rootEl = ref<HTMLElement | null>(null);

/**
 * Anchors the menu to the button rather than to the pointer.
 *
 * A menu tied to the control it belongs to stays put wherever within
 * the button the click landed, which matters for a 56px target whose
 * chevron is a 14px strip at one edge.
 */
function anchorRect(): { x: number; y: number } {
  const box = rootEl.value?.getBoundingClientRect();
  return { x: box?.left ?? 0, y: box?.bottom ?? 0 };
}

function openMenu() {
  if (!props.hasMenu) return;
  emit("menu", anchorRect());
}

function onBodyClick() {
  if (props.disabled) return;
  emit("run", anchorRect());
}

/**
 * A split button whose primary is disabled still opens its menu.
 *
 * "Convert" needs a selection but "Unpack to folder" may not, and a
 * dead chevron would hide the half that still works.
 */
function onChevronClick(event: MouseEvent) {
  event.stopPropagation();
  openMenu();
}

// Right-click anywhere opens the menu -- upstream `ToolBar
// .contextMenuEvent`, gui2/bars.py:188. Cheap, and it makes the whole
// 56px button a target for the dropdown rather than just the strip.
function onContextMenu(event: MouseEvent) {
  if (!props.hasMenu) return;
  event.preventDefault();
  openMenu();
}

const tooltip = computed(() => props.title ?? (props.compact ? props.label : undefined));

/** "Add Books options", "Virtual library options": never an empty or doubled name. */
const chevronName = computed(() => {
  const base = (props.menuLabel ?? (props.label || props.title || "More")).replace(/…$/, "").trim();
  return props.menuLabel ? base : `${base} options`;
});
</script>

<template>
  <div
    ref="rootEl"
    class="tb-button"
    :class="{ 'has-menu': hasMenu, disabled, active, compact }"
    @contextmenu="onContextMenu"
  >
    <button
      type="button"
      class="tb-body"
      :disabled="disabled"
      :title="tooltip"
      :aria-pressed="active ? 'true' : undefined"
      :aria-haspopup="bodyOpensMenu ? 'menu' : undefined"
      :aria-expanded="bodyOpensMenu ? (expanded ? 'true' : 'false') : undefined"
      @click="onBodyClick"
    >
      <AppIcon v-if="icon" class="tb-icon" :name="icon" :size="22" />
      <span v-if="!compact" class="tb-label">{{ label }}</span>
    </button>
    <button
      v-if="hasMenu"
      type="button"
      class="tb-chevron"
      :title="chevronName"
      :aria-label="chevronName"
      aria-haspopup="menu"
      :aria-expanded="expanded ? 'true' : 'false'"
      @click="onChevronClick"
    ><AppIcon name="chevron-down" :size="12" :stroke-width="2.25" /></button>
  </div>
</template>

<style scoped>
.tb-button {
  display: inline-flex;
  align-items: stretch;
  border-radius: var(--radius);
  min-width: 0;
}
.tb-button:hover:not(.disabled) {
  background: var(--bg-hover);
}
.tb-button.active {
  background: var(--bg-selected);
}

/*
  The body and chevron are separate buttons but must read as one
  control, so the shared chrome lives on the wrapper and both children
  are transparent.
*/
.tb-body,
.tb-chevron {
  background: none;
  border: none;
  border-radius: 0;
  height: auto;
  color: inherit;
  cursor: pointer;
  padding: 0;
}
.tb-body:hover:not(:disabled),
.tb-chevron:hover {
  background: none;
}

.tb-body {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--sp-1);
  padding: var(--sp-2) var(--sp-4);
  /* Sized to the label, not to a fixed width: "Find duplicates…" was cut to
     "Find duplicate…" by a 96px cap. The label still never wraps. */
  min-width: 56px;
  height: var(--tb-h);
}
.compact .tb-body {
  max-width: none;
  min-width: 0;
  padding: var(--sp-2) var(--sp-3);
  height: 32px;
}

.tb-icon {
  /* Lucide line art draws in currentColor, so it follows the theme. */
  flex-shrink: 0;
}

.tb-label {
  font-size: var(--fs-small);
  line-height: var(--lh-tight);
  /* Single line with an ellipsis rather than wrapping: a label that
     wraps changes the button's height, and one button growing taller
     drags the whole toolbar with it. */
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 100%;
}

.tb-chevron {
  display: flex;
  align-items: center;
  justify-content: center;
  /* 22px wide: the arrow is its own click target and 14px was too small to hit. */
  width: 22px;
  color: var(--fg-muted);
  flex-shrink: 0;
}
/* The divider appears on hover only -- always-on, it would make twelve
   buttons read as twenty-four. */
.tb-button:hover .tb-chevron {
  border-left: 1px solid var(--border);
}
.tb-chevron:hover {
  color: var(--fg);
}

.tb-button.disabled .tb-body {
  opacity: 0.45;
  cursor: default;
}
/* Note the chevron is deliberately *not* dimmed with the body: its
   menu still works when the primary action does not. */
</style>
