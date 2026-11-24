// Which books are selected, and the keyboard and mouse gestures that change it:
// click, Ctrl-click, Shift-click ranges, select all, and the arrow keys.

import { ref, type Ref } from "vue";

import type { BookWindow } from "./bookWindow";
import type { LibraryViewMode } from "./columns";
import { useScope } from "./useScope";
import { navigateIndex } from "./virtualList";

export interface SelectionDeps {
  win: BookWindow;
  /** The book open in the details panel. */
  selectedBookId: Ref<number | null>;
  viewMode: Ref<LibraryViewMode>;
  tableRef: Ref<{ pageRows(): number; scrollToIndex(index: number): void } | null>;
  gridRef: Ref<{ layout(): { columns: number; pageRows: number }; scrollToIndex(index: number): void } | null>;
}

export function useSelection({ win, selectedBookId, viewMode, tableRef, gridRef }: SelectionDeps) {
  const selectMode = ref(false);
  const selectedIds = ref<Set<number>>(new Set());
  /** What actions act on: the selection, else the open book. */
  const scope = useScope(selectedIds, selectedBookId);

  /** The row a shift-click range starts from: the last one clicked. */
  let selectAnchor: number | null = null;

  /** A plain click: this book alone is selected (and open in the details panel). */
  function selectOne(bookId: number) {
    if (selectedIds.value.size > 0) selectedIds.value = new Set();
    selectedBookId.value = bookId;
    selectAnchor = bookId;
  }

  /**
   * Ctrl-click (or a tick in select mode): add the book to the selection, or
   * take it out. Without select mode the open book counts as selected, so
   * Ctrl-clicking a second row gives two, as in a file manager.
   */
  function toggleSelected(bookId: number) {
    const next = new Set(selectedIds.value);
    if (!selectMode.value && next.size === 0 && selectedBookId.value !== null) next.add(selectedBookId.value);
    if (next.has(bookId)) next.delete(bookId);
    else next.add(bookId);
    selectedIds.value = next;
    selectAnchor = bookId;
    if (!selectMode.value) selectedBookId.value = next.has(bookId) ? bookId : ([...next].pop() ?? null);
  }

  /**
   * Shift-click: everything between the anchor and this row, loaded or not.
   * In select mode the range is added to the ticks; otherwise it replaces the
   * selection, as in a file manager.
   */
  function rangeSelect(bookId: number) {
    const anchor = selectAnchor ?? selectedBookId.value;
    const from = anchor === null ? -1 : win.indexOf(anchor);
    const to = win.indexOf(bookId);
    if (from < 0 || to < 0) {
      if (selectMode.value) toggleSelected(bookId);
      else selectOne(bookId);
      return;
    }
    const next = selectMode.value ? new Set(selectedIds.value) : new Set<number>();
    for (const id of win.idsBetween(from, to)) next.add(id);
    selectedIds.value = next;
    selectAnchor = anchor;
    if (!selectMode.value) selectedBookId.value = bookId;
  }

  /** Every matching book, by id: the whole list, not just what is loaded. */
  function selectAll() {
    selectedIds.value = new Set(win.ids.value);
  }

  function onCardClick(bookId: number, event?: MouseEvent) {
    if (event?.shiftKey) rangeSelect(bookId);
    else if (selectMode.value || event?.ctrlKey || event?.metaKey) toggleSelected(bookId);
    else selectOne(bookId);
  }

  /** Arrow/page/Home/End on the list: move the selected row and keep it in view. */
  function onNavigate(key: string, extend = false) {
    const n = win.ids.value.length;
    if (n === 0) return;
    const current = selectedBookId.value === null ? -1 : win.indexOf(selectedBookId.value);
    let next: number | null;
    if (viewMode.value === "table") {
      next = navigateIndex(key, current, n, tableRef.value?.pageRows() ?? 10);
    } else {
      const { columns, pageRows } = gridRef.value?.layout() ?? { columns: 1, pageRows: 3 };
      const clampTo = (i: number) => Math.min(n - 1, Math.max(0, i));
      const from = current < 0 ? 0 : current;
      if (current < 0 && key !== "Home" && key !== "End") next = 0;
      else if (key === "ArrowLeft") next = clampTo(from - 1);
      else if (key === "ArrowRight") next = clampTo(from + 1);
      else if (key === "ArrowUp") next = clampTo(from - columns);
      else if (key === "ArrowDown") next = clampTo(from + columns);
      else if (key === "PageUp") next = clampTo(from - columns * Math.max(1, pageRows - 1));
      else if (key === "PageDown") next = clampTo(from + columns * Math.max(1, pageRows - 1));
      else if (key === "Home") next = 0;
      else if (key === "End") next = n - 1;
      else next = null;
    }
    if (next === null) return;
    const id = win.idAt(next);
    if (id === undefined) return;
    if (extend && !selectMode.value) {
      // Shift+arrow grows the selection from where the walk started.
      const anchor = selectAnchor ?? selectedBookId.value ?? id;
      const from = win.indexOf(anchor);
      selectedIds.value = new Set(win.idsBetween(from < 0 ? next : from, next));
      selectAnchor = from < 0 ? id : anchor;
    } else if (!selectMode.value) {
      if (selectedIds.value.size > 0) selectedIds.value = new Set();
      selectAnchor = id;
    }
    selectedBookId.value = id;
    (viewMode.value === "table" ? tableRef.value : gridRef.value)?.scrollToIndex(next);
  }

  return { selectMode, selectedIds, scope, selectOne, toggleSelected, rangeSelect, selectAll, onCardClick, onNavigate };
}
