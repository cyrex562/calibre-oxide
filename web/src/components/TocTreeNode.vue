<script setup lang="ts">
import type { TocNode } from "../library/tweak";

// Real, recursive visual TOC tree editor (issue #760). Takes the
// WHOLE sibling list at this level (not a single node) via v-model, so
// add/remove/reorder can mutate it directly by index without needing
// a separate "which array am I in" plumbing layer -- each node's own
// `children` array is passed down as the next level's v-model.
const nodes = defineModel<TocNode[]>({ required: true });

function newNode(): TocNode {
  return { title: "New Entry", dest: null, frag: null, children: [] };
}

function addChild(i: number) {
  nodes.value[i].children.push(newNode());
}
function addSiblingAfter(i: number) {
  nodes.value.splice(i + 1, 0, newNode());
}
function remove(i: number) {
  nodes.value.splice(i, 1);
}
function moveUp(i: number) {
  if (i === 0) return;
  const arr = nodes.value;
  [arr[i - 1], arr[i]] = [arr[i], arr[i - 1]];
}
function moveDown(i: number) {
  if (i === nodes.value.length - 1) return;
  const arr = nodes.value;
  [arr[i], arr[i + 1]] = [arr[i + 1], arr[i]];
}
</script>

<template>
  <ul class="toc-list">
    <li v-for="(node, i) in nodes" :key="i" class="toc-node">
      <div class="toc-row">
        <input v-model="node.title" placeholder="Title" class="toc-title" />
        <input v-model="node.dest" placeholder="destination (e.g. chapter1.xhtml)" class="toc-dest" />
        <span v-if="node.dest_exists === false" class="toc-warning" title="This destination wasn't found in the book">⚠</span>
        <button type="button" :disabled="i === 0" @click="moveUp(i)" title="Move up">↑</button>
        <button type="button" :disabled="i === nodes.length - 1" @click="moveDown(i)" title="Move down">↓</button>
        <button type="button" @click="addChild(i)" title="Add a child entry">+ Child</button>
        <button type="button" @click="addSiblingAfter(i)" title="Add a sibling entry after this one">+ Sibling</button>
        <button type="button" class="danger" @click="remove(i)" title="Remove this entry (and any children)">✕</button>
      </div>
      <TocTreeNode v-if="node.children.length > 0" v-model="node.children" />
    </li>
  </ul>
</template>

<style scoped>
.toc-list {
  list-style: none;
  margin: 0;
  padding-left: 1.25em;
}
.toc-list:first-of-type {
  padding-left: 0;
}
.toc-row {
  display: flex;
  align-items: center;
  gap: 0.3em;
  margin: 0.2em 0;
}
.toc-title {
  flex: 1;
  min-width: 6em;
  font: inherit;
  padding: 0.25em 0.4em;
  border: 1px solid #ddd;
  border-radius: 4px;
}
.toc-dest {
  flex: 1;
  min-width: 6em;
  font: inherit;
  font-size: 0.85em;
  padding: 0.25em 0.4em;
  border: 1px solid #ddd;
  border-radius: 4px;
  color: #555;
}
.toc-warning {
  color: #b00020;
}
.toc-row button {
  font-size: 0.8em;
  padding: 0.2em 0.4em;
  cursor: pointer;
}
.toc-row button.danger {
  color: #b00020;
}
</style>
