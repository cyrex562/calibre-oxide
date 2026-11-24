// What an action acts on: the selected books when there are any, else the
// book that is open. Seven actions in the library view each carried their own
// copy of this expression; they drifted (some also required select mode).

import { computed, type ComputedRef, type Ref } from "vue";

/** The ids an action should act on, in selection order. */
export function scopeIds(selected: ReadonlySet<number>, currentId: number | null): number[] {
  if (selected.size > 0) return [...selected];
  return currentId !== null ? [currentId] : [];
}

export interface Scope {
  /** The selection, else the current book, else nothing. */
  ids: ComputedRef<number[]>;
  /** How many books `ids` holds. */
  count: ComputedRef<number>;
}

export function useScope(selected: Ref<Set<number>>, currentId: Ref<number | null>): Scope {
  const ids = computed(() => scopeIds(selected.value, currentId.value));
  const count = computed(() => ids.value.length);
  return { ids, count };
}
