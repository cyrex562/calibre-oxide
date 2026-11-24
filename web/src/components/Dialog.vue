<script setup lang="ts">
// The one modal dialog. Every dialog in the app is this component with its
// own content in the default slot (and its buttons in `footer`), so they all
// get the same behaviour:
//   - role="dialog" aria-modal="true" labelled by the title (the marker
//     `isModalOpen()` looks for, so library shortcuts stay quiet behind it);
//   - focus moves in when it opens (the first [autofocus] element, else the
//     first focusable one, else the panel), Tab / Shift+Tab stay inside, and
//     focus goes back to where it was when it closes;
//   - Escape closes it, but only the topmost of several open dialogs;
//   - a click on the backdrop closes it unless `busy`;
//   - with `dirty`, Escape and a backdrop click first ask "Discard changes?".
// It is teleported to <body>, so no ancestor's overflow or z-index clips it.
// The parent owns the open state: this only emits `close`.

import { nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, useId, watch } from "vue";

import { initialFocusTarget, isTopDialog, pushDialog, removeDialog, trapTab } from "../library/dialogStack";

defineOptions({ inheritAttrs: false });

const props = withDefaults(
  defineProps<{
    /** Omit when the dialog is mounted with v-if. */
    open?: boolean;
    title?: string;
    /** The accessible name for a dialog that draws its own heading. */
    label?: string;
    /** Work is running: the backdrop and Escape do not close it. */
    busy?: boolean;
    /** There are unsaved edits: closing by backdrop or Escape asks first. */
    dirty?: boolean;
    /** `bare` is a full-window surface that draws its own chrome (the image viewer). */
    size?: "sm" | "md" | "lg" | "full" | "bare";
    closeOnBackdrop?: boolean;
    /** Draw the ✕ button in the corner (it asks first when `dirty`, and is off while `busy`). */
    showClose?: boolean;
  }>(),
  { open: true, title: undefined, label: undefined, busy: false, dirty: false, size: "md", closeOnBackdrop: true, showClose: false },
);
const emit = defineEmits<{ close: [] }>();

const titleId = `dialog-title-${useId()}`;
const panel = ref<HTMLElement | null>(null);

let id: symbol | null = null;
let opener: Element | null = null;

function activate() {
  if (id !== null) return;
  opener = document.activeElement;
  id = pushDialog();
  document.addEventListener("keydown", onKeydown);
}

function deactivate(restoreFocus = true) {
  if (id === null) return;
  document.removeEventListener("keydown", onKeydown);
  removeDialog(id);
  id = null;
  const back = opener;
  opener = null;
  // Only a still-attached element can take focus back.
  if (restoreFocus && back instanceof HTMLElement && back.isConnected) back.focus();
}

async function focusIn() {
  await nextTick();
  if (panel.value === null || id === null) return;
  // Something inside already took focus (an autofocus attribute): leave it.
  if (document.activeElement !== null && document.activeElement !== document.body && panel.value.contains(document.activeElement)) return;
  initialFocusTarget(panel.value).focus({ preventScroll: true });
}

if (props.open) activate();
onMounted(() => {
  if (props.open) void focusIn();
});
watch(
  () => props.open,
  (open) => {
    if (open) {
      activate();
      void focusIn();
    } else {
      deactivate();
    }
  },
  { flush: "pre" },
);
onBeforeUnmount(() => deactivate());
// Parked by <keep-alive> (the library view while the reader is open): an open
// dialog must not keep the topmost slot, or it would swallow Escape and Tab.
onDeactivated(() => deactivate(false));
onActivated(() => {
  if (props.open) activate();
});

async function requestClose() {
  if (props.busy) return;
  if (props.dirty) {
    // Imported on use: the confirm is itself built on this component.
    const { confirmDialog } = await import("./dialogs/confirm");
    if (!(await confirmDialog("What you changed in this dialog will be lost.", { title: "Discard changes?", confirmLabel: "Discard", danger: true }))) return;
  }
  emit("close");
}

function onKeydown(event: KeyboardEvent) {
  if (id === null || !isTopDialog(id) || panel.value === null) return;
  if (event.key === "Escape" && !event.defaultPrevented && !event.isComposing) {
    event.preventDefault();
    void requestClose();
  } else if (event.key === "Tab" && !event.ctrlKey && !event.altKey && !event.metaKey) {
    if (trapTab(panel.value, event)) event.preventDefault();
  }
}

// A drag that starts on the text of the panel and ends on the backdrop is a
// text selection, not a click outside.
let pressedOnBackdrop = true;
function onBackdropDown(event: MouseEvent) {
  pressedOnBackdrop = event.target === event.currentTarget;
}
function onBackdropClick(event: MouseEvent) {
  const direct = event.target === event.currentTarget && pressedOnBackdrop;
  pressedOnBackdrop = true;
  if (direct && props.closeOnBackdrop) void requestClose();
}

// Another dialog is open above this one: the title bar greys out (Classic skin).
function isUnderneath(): boolean {
  return id !== null && !isTopDialog(id);
}

defineExpose({ requestClose });
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="dialog-backdrop" data-dialog-backdrop @mousedown="onBackdropDown" @click="onBackdropClick">
      <div
        ref="panel"
        v-bind="$attrs"
        class="dialog-panel manage-panel"
        :class="`dialog-${size}`"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="title ? titleId : undefined"
        :aria-label="title ? undefined : label"
        :aria-busy="busy || undefined"
        tabindex="-1"
      >
        <!-- The title bar. Under the Modern skin it is only the heading, with the ✕ in
             the corner when `showClose`; under Classic it is the navy bar, and the ✕ is
             always there, outside the Tab order like a window's own close box (hidden by
             `.close-auto` in Modern). -->
        <div v-if="size !== 'bare'" class="dialog-titlebar" :class="{ 'dialog-inactive': isUnderneath() }">
          <h3 v-if="title" :id="titleId" class="dialog-title">{{ title }}</h3>
          <span v-else-if="label" class="dialog-title dialog-title-label" aria-hidden="true">{{ label }}</span>
          <button type="button" class="manage-close" :class="{ 'close-auto': !showClose }" :tabindex="showClose ? undefined : -1" aria-label="Close" :disabled="busy" @click="requestClose">✕</button>
        </div>
        <div class="dialog-body"><slot /></div>
        <footer v-if="$slots.footer" class="dialog-footer"><slot name="footer" /></footer>
      </div>
    </div>
  </Teleport>
</template>

<style>
/* Unscoped on purpose: the panel is teleported and the slot content carries
   its owner's scope id, so only these frame rules live here. */
.dialog-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: var(--z-dialog);
}
.dialog-panel {
  box-sizing: border-box;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.3);
  gap: 0.75em;
}
.dialog-panel:focus {
  outline: none;
}
.dialog-panel.dialog-sm {
  width: min(420px, 94vw);
  max-width: none;
}
.dialog-panel.dialog-md {
  width: min(600px, 94vw);
  max-width: none;
}
.dialog-panel.dialog-lg {
  width: min(860px, 94vw);
  max-width: none;
}
.dialog-panel.dialog-full {
  width: 94vw;
  max-width: none;
  height: 90vh;
  max-height: 90vh;
}
.dialog-panel.dialog-bare {
  position: fixed;
  inset: 0;
  width: auto;
  max-width: none;
  max-height: none;
  padding: 0;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
  gap: 0;
  overflow: hidden;
}
.dialog-bare .dialog-body {
  gap: 0;
  overflow: hidden;
}
/* No box of its own under Modern: the heading and the ✕ lay out exactly as
   they did before the title bar existed. Classic draws it (skin-classic.css). */
.dialog-titlebar {
  display: contents;
}
.dialog-title {
  margin: 0;
  font-size: var(--fs-large);
}
.dialog-title-label,
.manage-close.close-auto {
  display: none;
}
.dialog-body {
  display: flex;
  flex-direction: column;
  gap: 1.25em;
  min-height: 0;
  overflow: auto;
  flex: 1 1 auto;
}
/* The button row every dialog ends with: right-aligned, one gap between
   buttons, the primary (or destructive) one last. A status line or a tick
   box in the footer sits on the left (`margin-right: auto`). */
.dialog-footer {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--sp-4);
  flex: 0 0 auto;
}
.dialog-footer > button.primary,
.dialog-footer > button.danger {
  order: 1;
}
.dialog-footer > .footer-note {
  margin-right: auto;
}
</style>
