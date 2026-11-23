<script setup lang="ts">
// Full-window image viewer: wheel / +/- zoom, drag to pan, Fit and 100%,
// Esc to close. Used for the cover and for a rendered PDF page, so a
// title page can be read. The maths is in library/imageViewer.ts.

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { actualSizeState, fitState, panBy, wheelFactor, zoomAt, type Size, type ViewState } from "../library/imageViewer";

const props = defineProps<{
  src: string;
  title: string;
  /** Shown beside the title, e.g. "Page 1 of 2". */
  caption?: string;
  /** Left/right arrows emit `step` when true. */
  steppable?: boolean;
  loading?: boolean;
}>();
const emit = defineEmits<{ close: []; step: [delta: number] }>();

const stage = ref<HTMLElement | null>(null);
const image = ref<Size>({ w: 0, h: 0 });
const view = ref<Size>({ w: 0, h: 0 });
const state = ref<ViewState>({ scale: 1, x: 0, y: 0 });
const userZoomed = ref(false);

function measure() {
  const el = stage.value;
  if (!el) return;
  view.value = { w: el.clientWidth, h: el.clientHeight };
  if (!userZoomed.value && image.value.w > 0) state.value = fitState(image.value, view.value);
}

function onLoad(event: Event) {
  const img = event.target as HTMLImageElement;
  image.value = { w: img.naturalWidth, h: img.naturalHeight };
  userZoomed.value = false;
  measure();
}

// A new picture (next book, next page) starts fitted again.
watch(
  () => props.src,
  () => {
    userZoomed.value = false;
    image.value = { w: 0, h: 0 };
  },
);

let observer: ResizeObserver | null = null;
onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && stage.value) {
    observer = new ResizeObserver(measure);
    observer.observe(stage.value);
  }
  window.addEventListener("keydown", onKey, true);
});
onBeforeUnmount(() => {
  observer?.disconnect();
  window.removeEventListener("keydown", onKey, true);
});

function zoom(factor: number, ax = view.value.w / 2, ay = view.value.h / 2) {
  if (image.value.w === 0) return;
  state.value = zoomAt(state.value, factor, ax, ay, image.value, view.value);
  userZoomed.value = true;
}

function fit() {
  userZoomed.value = false;
  if (image.value.w > 0) state.value = fitState(image.value, view.value);
}

function actualSize() {
  if (image.value.w === 0) return;
  state.value = actualSizeState(image.value, view.value);
  userZoomed.value = true;
}

function onWheel(event: WheelEvent) {
  event.preventDefault();
  const rect = stage.value?.getBoundingClientRect();
  zoom(wheelFactor(event.deltaY), event.clientX - (rect?.left ?? 0), event.clientY - (rect?.top ?? 0));
}

let drag: { x: number; y: number } | null = null;
const panning = ref(false);

function onPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  drag = { x: event.clientX, y: event.clientY };
  panning.value = true;
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
}
function onPointerMove(event: PointerEvent) {
  if (!drag) return;
  state.value = panBy(state.value, event.clientX - drag.x, event.clientY - drag.y, image.value, view.value);
  drag = { x: event.clientX, y: event.clientY };
  userZoomed.value = true;
}
function onPointerUp(event: PointerEvent) {
  drag = null;
  panning.value = false;
  (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
}

function onKey(event: KeyboardEvent) {
  switch (event.key) {
    case "Escape":
      event.preventDefault();
      event.stopPropagation();
      emit("close");
      break;
    case "+":
    case "=":
      event.preventDefault();
      zoom(1.25);
      break;
    case "-":
    case "_":
      event.preventDefault();
      zoom(1 / 1.25);
      break;
    case "0":
      event.preventDefault();
      fit();
      break;
    case "ArrowLeft":
    case "ArrowUp":
      if (props.steppable) {
        event.preventDefault();
        event.stopPropagation();
        emit("step", -1);
      }
      break;
    case "ArrowRight":
    case "ArrowDown":
      if (props.steppable) {
        event.preventDefault();
        event.stopPropagation();
        emit("step", 1);
      }
      break;
    default:
      break;
  }
}

const percent = computed(() => Math.round(state.value.scale * 100));
const imgStyle = computed(() => ({
  width: `${image.value.w}px`,
  height: `${image.value.h}px`,
  transform: `translate(${state.value.x}px, ${state.value.y}px) scale(${state.value.scale})`,
}));
</script>

<template>
  <div class="viewer" role="dialog" aria-modal="true" :aria-label="`Image viewer: ${title}`">
    <div class="bar">
      <span class="title">{{ title }}<span v-if="caption" class="caption"> · {{ caption }}</span></span>
      <span class="spacer"></span>
      <button v-if="steppable" type="button" title="Previous (left arrow)" aria-label="Previous" @click="emit('step', -1)">‹</button>
      <button v-if="steppable" type="button" title="Next (right arrow)" aria-label="Next" @click="emit('step', 1)">›</button>
      <button type="button" title="Zoom out (-)" aria-label="Zoom out" @click="zoom(1 / 1.25)">−</button>
      <span class="percent" data-testid="zoom-percent">{{ percent }}%</span>
      <button type="button" title="Zoom in (+)" aria-label="Zoom in" @click="zoom(1.25)">+</button>
      <button type="button" class="fit" @click="fit">Fit</button>
      <button type="button" class="actual" @click="actualSize">100%</button>
      <button type="button" class="close-viewer" title="Close (Esc)" aria-label="Close viewer" @click="emit('close')">✕</button>
    </div>
    <div ref="stage" class="stage" :class="{ panning }" @wheel="onWheel" @pointerdown="onPointerDown" @pointermove="onPointerMove" @pointerup="onPointerUp" @pointercancel="onPointerUp" @dblclick="userZoomed ? fit() : actualSize()">
      <p v-if="loading" class="status-line">Rendering…</p>
      <img v-if="src" :src="src" :alt="title" class="picture" draggable="false" :style="imgStyle" @load="onLoad" />
    </div>
  </div>
</template>

<style scoped>
.viewer {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: rgba(0, 0, 0, 0.92);
  display: flex;
  flex-direction: column;
  color: #fff;
}
.bar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.4rem 0.75rem;
  background: rgba(255, 255, 255, 0.08);
}
.bar button {
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
  border: 1px solid rgba(255, 255, 255, 0.25);
  border-radius: 4px;
  padding: 0.25rem 0.7rem;
  cursor: pointer;
  font: inherit;
}
.bar button:hover {
  background: rgba(255, 255, 255, 0.25);
}
.title {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.caption {
  font-weight: 400;
  opacity: 0.8;
}
.spacer {
  flex: 1;
}
.percent {
  min-width: 3.5em;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
.stage {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow: hidden;
  cursor: grab;
  touch-action: none;
}
.stage.panning {
  cursor: grabbing;
}
.picture {
  position: absolute;
  left: 0;
  top: 0;
  transform-origin: 0 0;
  max-width: none;
  user-select: none;
  image-rendering: auto;
}
.status-line {
  position: absolute;
  left: 50%;
  top: 1rem;
  transform: translateX(-50%);
  margin: 0;
  z-index: 1;
}
</style>
