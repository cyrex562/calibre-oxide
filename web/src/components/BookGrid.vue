<script setup lang="ts">
// The cover grid, virtualised by rows of cards: the number of cards per
// row follows the container width, and only the rows near the viewport
// are rendered (see library/virtualList.ts). Thumbnails are therefore
// requested only for cards that are actually on screen.

import { useKeepScroll } from "../library/keepScroll";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import type { BookSummary } from "../library/types";
import { gridMetrics, gridRowItems, gridRowOf, windowRange } from "../library/virtualList";

const FALLBACK_WIDTH = 900;
const FALLBACK_HEIGHT = 600;
const GAP = 16;

const props = defineProps<{
  ids: number[];
  getBook: (id: number) => BookSummary | undefined;
  selectMode: boolean;
  selectedIds: Set<number>;
  selectedBookId: number | null;
  markedIds: Set<number>;
  cacheBust: number;
  resetKey?: number;
}>();

const emit = defineEmits<{
  select: [bookId: number, event: MouseEvent];
  contextMenu: [value: { bookId: number; x: number; y: number }];
  toggleSelected: [bookId: number, event: MouseEvent];
  range: [value: { first: number; last: number }];
  navigate: [key: string, extend: boolean];
}>();

const scroller = ref<HTMLElement | null>(null);
useKeepScroll(scroller);
const width = ref(FALLBACK_WIDTH);
const height = ref(FALLBACK_HEIGHT);
const scrollTop = ref(0);

const metrics = computed(() => gridMetrics(width.value, props.ids.length, { minCardWidth: 120, gap: GAP, coverRatio: 1.5, captionHeight: 48 }));
const win = computed(() => windowRange({ count: metrics.value.rows, rowHeight: metrics.value.rowHeight, scrollTop: scrollTop.value, viewportHeight: height.value, overscan: 2 }));
const items = computed(() => gridRowItems(win.value.start, win.value.end, metrics.value.columns, props.ids.length));
const rendered = computed(() => {
  const out: number[] = [];
  for (let i = items.value.from; i < items.value.to; i++) out.push(i);
  return out;
});

watch(
  () => [items.value.from, items.value.to] as const,
  ([first, last]) => emit("range", { first, last }),
  { immediate: true },
);

function measure() {
  const el = scroller.value;
  if (!el) return;
  // Content width excludes the scrollbar, which clientWidth already does.
  if (el.clientWidth > 0) width.value = el.clientWidth;
  if (el.clientHeight > 0) height.value = el.clientHeight;
}

let observer: ResizeObserver | null = null;
onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && scroller.value) {
    observer = new ResizeObserver(measure);
    observer.observe(scroller.value);
  }
});
onBeforeUnmount(() => observer?.disconnect());

function onScroll() {
  if (scroller.value) scrollTop.value = scroller.value.scrollTop;
}

watch(
  () => props.resetKey,
  () => {
    if (scroller.value) scroller.value.scrollTop = 0;
    scrollTop.value = 0;
  },
);

function scrollToIndex(index: number) {
  const el = scroller.value;
  if (!el) return;
  const rowTop = gridRowOf(index, metrics.value.columns) * metrics.value.rowHeight;
  const viewport = el.clientHeight || height.value;
  let next = el.scrollTop;
  if (rowTop < next) next = rowTop;
  else if (rowTop + metrics.value.rowHeight > next + viewport) next = rowTop + metrics.value.rowHeight - viewport;
  if (next !== el.scrollTop) el.scrollTop = next;
  scrollTop.value = el.scrollTop;
}

/** Cards per row and rows on screen, for the page keys and left/right movement. */
function layout() {
  return { columns: metrics.value.columns, pageRows: Math.max(1, Math.floor(height.value / metrics.value.rowHeight)) };
}

defineExpose({ scrollToIndex, layout });

const NAV_KEYS = new Set(["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "PageDown", "PageUp", "Home", "End"]);

function onKeydown(event: KeyboardEvent) {
  if (event.ctrlKey || event.metaKey || event.altKey || !NAV_KEYS.has(event.key)) return;
  const target = event.target as HTMLElement | null;
  if (target && (target.tagName === "INPUT" || target.tagName === "SELECT")) return;
  event.preventDefault();
  emit("navigate", event.key, event.shiftKey);
  scroller.value?.focus({ preventScroll: true });
}
</script>

<template>
  <div ref="scroller" class="grid-scroll" tabindex="-1" @scroll.passive="onScroll" @keydown="onKeydown">
    <div class="grid-pad" :style="{ height: `${win.padTop}px` }"></div>
    <div class="grid-rows" :style="{ gridTemplateColumns: `repeat(${metrics.columns}, minmax(0, 1fr))`, columnGap: `${GAP}px`, gridAutoRows: `${metrics.rowHeight}px` }">
      <template v-for="i in rendered" :key="ids[i]">
        <button
          v-if="getBook(ids[i])"
          class="card"
          :class="{ selected: selectedIds.has(ids[i]) || (!selectMode && selectedBookId === ids[i]) }"
          @click="emit('select', ids[i], $event)"
          @contextmenu.prevent="emit('contextMenu', { bookId: ids[i], x: $event.clientX, y: $event.clientY })"
        >
          <input v-if="selectMode" type="checkbox" class="card-checkbox" :checked="selectedIds.has(ids[i])" @click.stop="emit('toggleSelected', ids[i], $event)" />
          <span v-if="markedIds.has(ids[i])" class="card-mark" title="Marked">●</span>
          <span class="cover" :style="{ height: `${metrics.cardWidth * 1.5}px` }">
            <img :src="`${getBook(ids[i])!.thumbnail}?v=${cacheBust}`" :alt="getBook(ids[i])!.title" loading="lazy" />
          </span>
          <div class="card-title">{{ getBook(ids[i])!.title }}</div>
          <div class="card-authors">{{ (getBook(ids[i])!.authors ?? []).join(" & ") }}</div>
        </button>
        <div v-else class="card placeholder" aria-hidden="true">
          <span class="cover" :style="{ height: `${metrics.cardWidth * 1.5}px` }"></span>
          <div class="card-title">&nbsp;</div>
        </div>
      </template>
    </div>
    <div class="grid-pad" :style="{ height: `${win.padBottom}px` }"></div>
  </div>
</template>

<style scoped>
.grid-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.grid-scroll:focus {
  outline: none;
}
.grid-rows {
  display: grid;
}
.card {
  background: none;
  border: none;
  /* The global `button` rule fixes a control height. */
  height: auto;
  cursor: pointer;
  text-align: left;
  padding: 0;
  font: inherit;
  color: inherit;
  /* Also the containing block for `.card-mark` and `.card-checkbox`. */
  position: relative;
  min-width: 0;
  overflow: hidden;
  /* Cards are a fixed height (the row height) so rows can be positioned
     from the scroll offset; the grid row gap is built into rowHeight. */
  align-self: start;
}
.cover {
  display: block;
  width: 100%;
  background: var(--border);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
}
.card.placeholder {
  cursor: default;
  opacity: 0.45;
}
.card img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.card.selected .cover {
  outline: 3px solid var(--accent);
  outline-offset: -3px;
}
.card-checkbox {
  position: absolute;
  top: 0.3em;
  left: 0.3em;
  width: 1.1em;
  height: 1.1em;
  z-index: 1;
}
.card-mark {
  position: absolute;
  top: 4px;
  left: 6px;
  z-index: 1;
  color: var(--mark);
  font-size: var(--fs-body);
  text-shadow: 0 0 3px rgb(0 0 0 / 45%);
}
.card-title {
  font-weight: 600;
  font-size: var(--fs-small);
  margin-top: 0.25em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-authors {
  font-size: var(--fs-small);
  color: var(--fg-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
