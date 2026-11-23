<script setup lang="ts">
// One collapsible section of the details panel: a clickable header with
// a chevron over a body that is only rendered while open.
defineProps<{ title: string; open: boolean; id: string }>();
const emit = defineEmits<{ toggle: [] }>();
</script>

<template>
  <section class="section" :class="{ open }" :data-section="id">
    <h3 class="section-head">
      <button type="button" class="section-toggle" :aria-expanded="open" :aria-controls="`section-${id}`" @click="emit('toggle')">
        <span class="chevron" aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
        {{ title }}
        <span class="head-extra"><slot name="head" /></span>
      </button>
    </h3>
    <div v-if="open" :id="`section-${id}`" class="section-body"><slot /></div>
  </section>
</template>

<style scoped>
.section {
  border-top: 1px solid var(--border);
}
.section-head {
  margin: 0;
  font-size: var(--fs-body);
}
.section-toggle {
  all: unset;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 0.4rem;
  width: 100%;
  padding: 0.5rem 0.25rem;
  cursor: pointer;
  font-weight: 600;
  color: var(--fg-muted);
}
.section-toggle:hover {
  color: var(--fg);
  background: var(--accent-soft);
}
.section-toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.chevron {
  width: 1em;
  text-align: center;
}
.head-extra {
  margin-left: auto;
  font-weight: 400;
}
.section-body {
  padding: 0.25rem 0.25rem 0.75rem;
}
</style>
