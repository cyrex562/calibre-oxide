// The selection gestures, on their own: click, Ctrl-click, Shift-click and
// Shift+arrow, in and out of select mode, over a list whose rows may not all be
// loaded.

import { ref } from "vue";
import { describe, expect, it } from "vitest";

import { createBookWindow } from "./bookWindow";
import type { LibraryViewMode } from "./columns";
import type { BookSummary } from "./types";
import { useSelection } from "./useSelection";

async function setup(count = 10) {
  const ids = Array.from({ length: count }, (_, i) => i + 1);
  const win = createBookWindow({ fetchIds: async () => ({ ids, total: ids.length }), fetchBooks: async (want) => want.map((id) => ({ id, title: `Book ${id}` }) as unknown as BookSummary) });
  await win.reset();
  const selectedBookId = ref<number | null>(null);
  const viewMode = ref<LibraryViewMode>("table");
  const scrolled: number[] = [];
  const tableRef = ref({ pageRows: () => 5, scrollToIndex: (i: number) => scrolled.push(i) });
  const gridRef = ref(null);
  const s = useSelection({ win, selectedBookId, viewMode, tableRef, gridRef });
  return { ...s, selectedBookId, scrolled, win };
}

describe("useSelection", () => {
  it("a plain click opens one book and clears the selection", async () => {
    const s = await setup();
    s.toggleSelected(2);
    s.toggleSelected(3);
    s.selectOne(5);
    expect([...s.selectedIds.value]).toEqual([]);
    expect(s.selectedBookId.value).toBe(5);
  });

  it("Ctrl-click counts the open book as selected, so a second row makes two", async () => {
    const s = await setup();
    s.selectOne(1);
    s.toggleSelected(4);
    expect([...s.selectedIds.value].sort()).toEqual([1, 4]);
    s.toggleSelected(1);
    expect([...s.selectedIds.value]).toEqual([4]);
    expect(s.selectedBookId.value).toBe(4);
  });

  it("Shift-click selects everything between the anchor and the row, loaded or not", async () => {
    const s = await setup(50);
    s.selectOne(10);
    s.rangeSelect(40);
    expect(s.selectedIds.value.size).toBe(31);
    expect(s.selectedIds.value.has(25)).toBe(true);
    expect(s.selectedBookId.value).toBe(40);
  });

  it("in select mode a range adds to the ticks and a click toggles one", async () => {
    const s = await setup();
    s.selectMode.value = true;
    s.onCardClick(2);
    s.onCardClick(8);
    s.rangeSelect(4);
    // The range runs from the last click (8) to the row, and is added to the ticks.
    expect([...s.selectedIds.value].sort((a, b) => a - b)).toEqual([2, 4, 5, 6, 7, 8]);
    s.onCardClick(8);
    expect(s.selectedIds.value.has(8)).toBe(false);
  });

  it("selectAll takes every id, not just the loaded rows", async () => {
    const s = await setup(30);
    s.selectAll();
    expect(s.selectedIds.value.size).toBe(30);
  });

  it("arrow keys move the open book and scroll to it; Shift grows a selection from where the walk began", async () => {
    const s = await setup();
    s.onNavigate("ArrowDown");
    expect(s.selectedBookId.value).toBe(1);
    s.onNavigate("ArrowDown");
    s.onNavigate("ArrowDown");
    expect(s.selectedBookId.value).toBe(3);
    expect(s.scrolled[s.scrolled.length - 1]).toBe(2);
    s.onNavigate("ArrowDown", true);
    expect([...s.selectedIds.value].sort((a, b) => a - b)).toEqual([3, 4]);
    s.onNavigate("ArrowDown", true);
    expect([...s.selectedIds.value].sort((a, b) => a - b)).toEqual([3, 4, 5]);
    s.onNavigate("End");
    expect(s.selectedBookId.value).toBe(10);
  });

  it("the scope is the selection, else the open book", async () => {
    const s = await setup();
    s.selectOne(7);
    expect(s.scope.ids.value).toEqual([7]);
    s.toggleSelected(2);
    expect([...s.scope.ids.value].sort((a, b) => a - b)).toEqual([2, 7]);
  });
});
