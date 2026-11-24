<script setup lang="ts">
// A text input that suggests names the library already has (authors,
// tags, series, publishers, custom-column values) as you type.
//
// It is a drop-in for a plain `<input type="text">`: `v-model` plus
// every other attribute, class and listener lands on the input itself.
//
// Key rules (kept deliberately conservative so existing save/cancel
// behaviour never changes unless the user is actually using the list):
//  - Nothing is highlighted when the list opens. The user has to press
//    Down first, so typing a brand-new name and pressing Enter saves
//    what was typed, never a suggestion the user did not choose.
//  - Down/Up move the highlight (and Down reopens a closed list).
//  - Enter or Tab accept the highlighted item, but ONLY when the list is
//    open and an item is highlighted. That key is then consumed
//    (preventDefault + stopPropagation + stopImmediatePropagation) so a
//    parent's Enter-to-save / Tab-to-move does not also fire.
//    Otherwise the key goes on to the parent's own handler untouched.
//  - Esc closes an open list and is consumed; with the list closed it
//    passes through, so a second Esc cancels a cell edit.
//  - A mouse click on a row accepts it; mousedown is cancelled so the
//    input keeps focus and does not blur-save first.
//
// The dropdown is teleported to <body> and positioned `fixed` from the
// input's rectangle, so a scrolling table cell cannot clip it.

import { computed, nextTick, onBeforeUnmount, onMounted, ref, useAttrs, watch } from "vue";
import { applySuggestion, getFieldValues, getMinChars, suggest, type FieldValue, type Suggestion } from "../library/fieldValues";

defineOptions({ inheritAttrs: false });

const props = defineProps<{
  modelValue: string;
  /** Category name to complete from (`authors`, `#genre`, ...). */
  field?: string;
  /** A fixed list to complete from instead of fetching one (enumerations). */
  options?: string[];
  /** `&` or `,` for fields holding several values; omit for single-value fields. */
  separator?: string | null;
  /** Characters needed before suggesting; defaults to the user's setting. */
  minChars?: number;
  /** Focus (and select) the input when it mounts. */
  focusOnMount?: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const attrs = useAttrs();

let nextId = 0;
const uid = `ac-${++nextId}-${Math.random().toString(36).slice(2, 7)}`;
const listId = `${uid}-list`;

const input = ref<HTMLInputElement | null>(null);
const list = ref<HTMLElement | null>(null);
const items = ref<Suggestion[]>([]);
const active = ref(-1);
const open = ref(false);
const pos = ref<{ left: number; top?: number; bottom?: number; width: number; maxHeight: number }>({ left: 0, width: 0, maxHeight: 280 });
let focused = false;
let requestToken = 0;

const inputAttrs = computed(() => {
  const { onKeydown: _k, onBlur: _b, onFocus: _f, onInput: _i, ...rest } = attrs as Record<string, unknown>;
  return rest;
});

function callAttr(name: "onKeydown" | "onBlur" | "onFocus" | "onInput", event: Event) {
  const h = (attrs as Record<string, unknown>)[name];
  if (typeof h === "function") h(event);
  else if (Array.isArray(h)) for (const f of h) if (typeof f === "function") f(event);
}

async function valuesFor(): Promise<FieldValue[]> {
  if (props.options) return props.options.map((name) => ({ name, count: 0 }));
  if (props.field) return getFieldValues(props.field);
  return [];
}

async function refresh(text: string, highlightFirst = false) {
  const token = ++requestToken;
  const values = await valuesFor();
  if (token !== requestToken || !focused) return;
  items.value = suggest(values, text, { separator: props.separator ?? null, minChars: props.minChars ?? getMinChars() });
  active.value = highlightFirst && items.value.length > 0 ? 0 : -1;
  open.value = items.value.length > 0;
  if (open.value) await place();
}

function close() {
  open.value = false;
  active.value = -1;
}

async function place() {
  await nextTick();
  const el = input.value;
  if (!el) return;
  const r = el.getBoundingClientRect();
  const vh = window.innerHeight;
  const want = Math.min(280, items.value.length * 30 + 8);
  const below = vh - r.bottom - 8;
  const above = r.top - 8;
  const flip = below < want && above > below;
  pos.value = flip
    ? { left: r.left, bottom: vh - r.top, width: Math.max(r.width, 180), maxHeight: Math.min(280, above) }
    : { left: r.left, top: r.bottom, width: Math.max(r.width, 180), maxHeight: Math.min(280, Math.max(below, 80)) };
}

function onInput(event: Event) {
  const text = (event.target as HTMLInputElement).value;
  emit("update:modelValue", text);
  callAttr("onInput", event);
  void refresh(text);
}

function onFocus(event: FocusEvent) {
  focused = true;
  callAttr("onFocus", event);
}

function onBlur(event: FocusEvent) {
  focused = false;
  requestToken++;
  close();
  callAttr("onBlur", event);
}

async function accept(index: number) {
  const s = items.value[index];
  if (!s) return;
  const text = applySuggestion(props.modelValue, s.name, props.separator ?? null);
  emit("update:modelValue", text);
  close();
  items.value = [];
  await nextTick();
  const el = input.value;
  if (el) {
    el.focus();
    el.setSelectionRange(text.length, text.length);
  }
}

function consume(event: KeyboardEvent) {
  event.preventDefault();
  event.stopPropagation();
  event.stopImmediatePropagation();
}

function onKeydown(event: KeyboardEvent) {
  if (event.isComposing || event.ctrlKey || event.metaKey || event.altKey) return callAttr("onKeydown", event);
  const n = items.value.length;
  switch (event.key) {
    case "ArrowDown":
      if (open.value) {
        event.preventDefault();
        active.value = (active.value + 1) % n;
        return;
      }
      if (n > 0 || props.field || props.options) {
        event.preventDefault();
        void refresh(props.modelValue, true);
        return;
      }
      break;
    case "ArrowUp":
      if (open.value) {
        event.preventDefault();
        active.value = active.value <= 0 ? n - 1 : active.value - 1;
        return;
      }
      break;
    case "Enter":
    case "Tab":
      if (open.value && active.value >= 0 && !(event.key === "Tab" && event.shiftKey)) {
        consume(event);
        void accept(active.value);
        return;
      }
      break;
    case "Escape":
      if (open.value) {
        consume(event);
        close();
        return;
      }
      break;
  }
  callAttr("onKeydown", event);
}

function onRepositionEvent() {
  if (open.value) void place();
}

watch(active, async (i) => {
  if (i < 0) return;
  await nextTick();
  list.value?.children[i]?.scrollIntoView?.({ block: "nearest" });
});

onMounted(() => {
  if (props.focusOnMount && input.value) {
    input.value.focus();
    input.value.select();
  }
  window.addEventListener("resize", onRepositionEvent);
  window.addEventListener("scroll", onRepositionEvent, true);
});
onBeforeUnmount(() => {
  requestToken++;
  window.removeEventListener("resize", onRepositionEvent);
  window.removeEventListener("scroll", onRepositionEvent, true);
});

const dropdownStyle = computed(() => ({
  left: `${pos.value.left}px`,
  width: `${pos.value.width}px`,
  maxHeight: `${pos.value.maxHeight}px`,
  ...(pos.value.top !== undefined ? { top: `${pos.value.top}px` } : { bottom: `${pos.value.bottom}px` }),
}));

/** The row's text split around the matched part. */
function parts(s: Suggestion): [string, string, string] {
  if (!s.range) return [s.name, "", ""];
  return [s.name.slice(0, s.range[0]), s.name.slice(s.range[0], s.range[1]), s.name.slice(s.range[1])];
}
</script>

<template>
  <input
    ref="input"
    v-bind="inputAttrs"
    type="text"
    :value="modelValue"
    role="combobox"
    autocomplete="off"
    aria-autocomplete="list"
    :aria-expanded="open"
    :aria-controls="listId"
    :aria-activedescendant="active >= 0 ? `${uid}-opt-${active}` : undefined"
    @input="onInput"
    @keydown="onKeydown"
    @focus="onFocus"
    @blur="onBlur"
  />
  <Teleport to="body">
    <ul v-show="open" :id="listId" ref="list" class="ac-list" role="listbox" :style="dropdownStyle">
      <li
        v-for="(s, i) in items"
        :id="`${uid}-opt-${i}`"
        :key="s.name"
        class="ac-item"
        :class="{ active: i === active }"
        role="option"
        :aria-selected="i === active"
        @mousedown.prevent="accept(i)"
        @mousemove="active = i"
      >
        <span class="ac-name">{{ parts(s)[0] }}<mark v-if="parts(s)[1]">{{ parts(s)[1] }}</mark>{{ parts(s)[2] }}</span>
        <span v-if="s.count > 0" class="ac-count">{{ s.count }}</span>
      </li>
    </ul>
  </Teleport>
</template>

<style>
.ac-list {
  position: fixed;
  z-index: var(--z-suggest);
  margin: 0;
  padding: 2px 0;
  list-style: none;
  overflow-y: auto;
  box-sizing: border-box;
  background: var(--bg-raised);
  color: var(--fg);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-menu);
  font-size: var(--fs-body);
}
.ac-item {
  display: flex;
  justify-content: space-between;
  gap: 0.75rem;
  padding: 4px 8px;
  cursor: pointer;
}
.ac-item.active {
  background: var(--accent);
  color: var(--fg-on-accent);
}
.ac-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ac-name mark {
  background: none;
  color: inherit;
  font-weight: 700;
  text-decoration: underline;
}
.ac-count {
  flex: none;
  opacity: 0.6;
  font-size: 0.85em;
}
</style>
