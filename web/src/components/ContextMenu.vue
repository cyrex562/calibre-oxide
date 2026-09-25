<script setup lang="ts">
// Right-click menu over the action registry (issue 1.2 of the #816
// epic).
//
// Presentational: it renders whatever entries it is given and reports
// which one was chosen. Deciding *which* actions belong in the menu,
// and whether each is enabled, is the registry's job (library/
// actions.ts) -- the same source the toolbar and the desktop native
// menu read, which is the whole point of having built it.

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import type { LibraryActionId } from "../library/actions";

export interface ContextMenuEntry {
  id: LibraryActionId;
  label: string;
  enabled: boolean;
  /** Starts a visual group; rendered as a separator above this entry. */
  startsGroup?: boolean;
}

const props = defineProps<{
  /** Viewport coordinates of the click that opened the menu. */
  x: number;
  y: number;
  entries: ContextMenuEntry[];
}>();

const emit = defineEmits<{ choose: [id: LibraryActionId]; close: [] }>();

const root = ref<HTMLElement | null>(null);
const position = ref({ left: props.x, top: props.y });

/**
 * Keeps the menu on screen. A right-click near the bottom or right
 * edge is completely normal, and a menu that opens off-screen there is
 * unusable -- so it flips back inside rather than extending the page.
 */
function reposition() {
  const el = root.value;
  if (!el) return;
  const { width, height } = el.getBoundingClientRect();
  const margin = 4;
  const left = Math.max(margin, Math.min(props.x, window.innerWidth - width - margin));
  const top = Math.max(margin, Math.min(props.y, window.innerHeight - height - margin));
  position.value = { left, top };
}

const enabledEntries = computed(() => props.entries.filter((e) => e.enabled));

function choose(entry: ContextMenuEntry) {
  if (!entry.enabled) return;
  emit("choose", entry.id);
  emit("close");
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.stopPropagation();
    emit("close");
  }
}

// `pointerdown` rather than `click`: a click elsewhere should dismiss
// the menu *before* that click activates whatever it landed on, which
// is what people expect from a native menu.
function onPointerDown(event: PointerEvent) {
  if (!root.value?.contains(event.target as Node)) emit("close");
}

onMounted(() => {
  reposition();
  root.value?.focus();
  window.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("keydown", onKeydown, true);
  window.addEventListener("resize", reposition);
  window.addEventListener("scroll", () => emit("close"), { once: true, capture: true });
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onPointerDown, true);
  window.removeEventListener("keydown", onKeydown, true);
  window.removeEventListener("resize", reposition);
});

watch(() => [props.x, props.y, props.entries], reposition, { flush: "post" });
</script>

<template>
  <div
    ref="root"
    class="context-menu"
    role="menu"
    tabindex="-1"
    :style="{ left: `${position.left}px`, top: `${position.top}px` }"
    @contextmenu.prevent
  >
    <p v-if="enabledEntries.length === 0" class="empty" role="none">No actions available</p>
    <template v-for="entry in entries" :key="entry.id">
      <hr v-if="entry.startsGroup" class="sep" role="separator" />
      <button type="button" role="menuitem" class="item" :disabled="!entry.enabled" @click="choose(entry)">
        {{ entry.label }}
      </button>
    </template>
  </div>
</template>

<style scoped>
.context-menu {
  position: fixed;
  z-index: 1000;
  min-width: 190px;
  padding: 0.25rem;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 6px 20px rgb(0 0 0 / 18%);
  display: flex;
  flex-direction: column;
}

.context-menu:focus {
  outline: none;
}

.item {
  all: unset;
  cursor: pointer;
  padding: 0.35rem 0.7rem;
  border-radius: 4px;
  font-size: var(--fs-body);
  white-space: nowrap;
}

.item:hover:not(:disabled),
.item:focus-visible {
  background: var(--accent-soft);
}

.item:disabled {
  opacity: 0.45;
  cursor: default;
}

.sep {
  border: none;
  border-top: 1px solid var(--border);
  margin: 0.25rem 0.3rem;
}

.empty {
  margin: 0;
  padding: 0.35rem 0.7rem;
  font-size: var(--fs-small);
  opacity: 0.6;
}

</style>
