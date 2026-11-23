<script setup lang="ts">
// One Lucide icon by the app's icon name (see `src/icons.ts`).
// Draws in `currentColor`, so it follows the theme in light and dark.
import { computed } from "vue";
import { iconComponent } from "../icons";

const props = withDefaults(defineProps<{ name?: string | null; size?: number; strokeWidth?: number }>(), {
  size: 24,
  strokeWidth: 1.75,
});
const comp = computed(() => iconComponent(props.name));
</script>

<template>
  <component :is="comp" v-if="comp" class="app-icon" :size="size" :stroke-width="strokeWidth" aria-hidden="true" />
</template>

<style scoped>
.app-icon {
  flex-shrink: 0;
}
</style>
