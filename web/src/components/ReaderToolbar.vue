<script setup lang="ts">
// The toolbar both readers share (U25).
//
// The EPUB and PDF readers each had their own header with different
// buttons in different places. What they have in common -- leaving,
// contents, bookmarks, search, adding a bookmark, previous/next -- is
// here once, in one order; what is particular to a format goes in the
// `position` slot (between Prev and Next) and the `extra` slot (the end
// of the bar).

defineProps<{
  title: string;
  /** Disables everything that needs a loaded book. */
  disabled?: boolean;
  bookmarkCount: number;
  contentsActive?: boolean;
  bookmarksActive?: boolean;
  searchActive?: boolean;
  canPrev: boolean;
  canNext: boolean;
}>();

defineEmits<{
  contents: [];
  bookmarks: [];
  search: [];
  bookmark: [];
  prev: [];
  next: [];
}>();
</script>

<template>
  <header class="reader-toolbar" role="toolbar" aria-label="Reader">
    <router-link to="/" class="back">Library</router-link>
    <button type="button" :disabled="disabled" :class="{ active: contentsActive }" :aria-pressed="!!contentsActive" @click="$emit('contents')">Contents</button>
    <button type="button" :disabled="disabled" :class="{ active: bookmarksActive }" :aria-pressed="!!bookmarksActive" @click="$emit('bookmarks')">Bookmarks ({{ bookmarkCount }})</button>
    <button type="button" :disabled="disabled" :class="{ active: searchActive }" :aria-pressed="!!searchActive" title="Search in this book (Ctrl+F)" @click="$emit('search')">Search</button>
    <button type="button" :disabled="disabled" @click="$emit('bookmark')">Bookmark this page</button>
    <span class="reader-sep" aria-hidden="true" />
    <button type="button" :disabled="disabled || !canPrev" @click="$emit('prev')">◀ Prev</button>
    <slot name="position" />
    <button type="button" :disabled="disabled || !canNext" @click="$emit('next')">Next ▶</button>
    <span class="reader-title" :title="title">{{ title }}</span>
    <slot name="extra" />
  </header>
</template>

<style scoped>
.reader-toolbar {
  display: flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.5em;
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}
.reader-title {
  flex: 1;
  min-width: 6rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.reader-sep {
  width: 1px;
  align-self: stretch;
  margin: 0 0.2rem;
  background: var(--border);
}
.reader-toolbar :deep(button.active) {
  background: var(--accent);
  color: var(--fg-on-accent);
  border-color: var(--accent);
}
</style>
