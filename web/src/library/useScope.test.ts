import { describe, expect, it } from "vitest";
import { ref } from "vue";

import { scopeIds, useScope } from "./useScope";

describe("scopeIds", () => {
  it("is the selection when there is one, else the open book, else nothing", () => {
    expect(scopeIds(new Set([3, 4]), 9)).toEqual([3, 4]);
    expect(scopeIds(new Set(), 9)).toEqual([9]);
    expect(scopeIds(new Set(), null)).toEqual([]);
  });
});

describe("useScope", () => {
  it("follows the selection and the open book", () => {
    const selected = ref(new Set<number>());
    const current = ref<number | null>(null);
    const scope = useScope(selected, current);
    expect(scope.count.value).toBe(0);
    current.value = 5;
    expect(scope.ids.value).toEqual([5]);
    selected.value = new Set([1, 2, 3]);
    expect(scope.ids.value).toEqual([1, 2, 3]);
    expect(scope.count.value).toBe(3);
  });
});
