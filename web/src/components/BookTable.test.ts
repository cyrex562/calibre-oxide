// Real render tests for the table (issue 1.1 of the #816 epic).
//
// Phase 0 shipped a registry-driven toolbar that nothing could
// actually render in CI, because this box cannot run a browser and the
// project has no component-test harness. That gap is worth closing
// here rather than repeating: a table is the library's primary view,
// and "the pure functions are right" says nothing about whether any
// rows reach the screen.
//
// No `@vue/test-utils` dependency needed -- Vue's own `createApp`
// mounts into a jsdom element perfectly well, and the component takes
// plain props with no router, store or network behind it.

import { createApp, h, nextTick, shallowReactive, type App } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import BookTable from "./BookTable.vue";
import { columnsFor, resolveColumns, DEFAULT_TABLE_PREFS, type BookColumn } from "../library/columns";
import type { BookFieldChanges, BookSummary } from "../library/types";
import { commitCellEdit } from "../library/inlineEdit";
import { getFieldValues, invalidateFieldValues } from "../library/fieldValues";

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

function book(id: number, overrides: Partial<BookSummary> = {}): BookSummary {
  return {
    id,
    title: `Book ${id}`,
    authors: ["Ann Lee"],
    series: null,
    series_index: 1,
    rating: null,
    tags: ["alpha"],
    pubdate: null,
    timestamp: "2026-09-21 12:20:34",
    last_modified: null,
    cover: `/get/cover/${id}`,
    thumbnail: `/get/thumb/${id}`,
    formats: ["pdf"],
    main_format: { pdf: `/get/pdf/${id}` },
    other_formats: {},
    publisher: "Acme",
    ...overrides,
  } as BookSummary;
}

const COLUMNS: BookColumn[] = resolveColumns(columnsFor({}, [["title", "Title"], ["authors", "Authors"], ["timestamp", "Date added"]]), DEFAULT_TABLE_PREFS);

interface Emitted {
  select: number[];
  toggleSelected: number[];
  sortBy: { sort: string; order: string }[];
  resize: { key: string; width: number }[];
}

function mount(props: Record<string, unknown> = {}) {
  host = document.createElement("div");
  document.body.appendChild(host);

  const emitted: Emitted = { select: [], toggleSelected: [], sortBy: [], resize: [] };

  app = createApp(BookTable, {
    ids: [1, 2],
    getBook: (id: number) => book(id),
    columns: COLUMNS,
    selectMode: false,
    selectedIds: new Set<number>(),
    selectedBookId: null,
    sort: "timestamp",
    sortOrder: "desc" as const,
    onSelect: (id: number) => emitted.select.push(id),
    onToggleSelected: (id: number) => emitted.toggleSelected.push(id),
    onSortBy: (v: { sort: string; order: string }) => emitted.sortBy.push(v),
    onResize: (v: { key: string; width: number }) => emitted.resize.push(v),
    ...props,
  });
  app.mount(host);
  return { el: host, emitted };
}

describe("rendering", () => {
  it("renders a row per book and a cell per column", () => {
    const { el } = mount();
    const rows = el.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(rows[0].querySelectorAll("td")).toHaveLength(COLUMNS.length);
  });

  it("puts real book data in the cells", () => {
    const { el } = mount();
    const text = el.querySelector("tbody tr")?.textContent ?? "";
    expect(text).toContain("Book 1");
    expect(text).toContain("Ann Lee");
    expect(text).toContain("Acme");
    // `formats` is lowercase in the row and upper-cased for display.
    expect(text).toContain("PDF");
  });

  it("renders a header per column", () => {
    const { el } = mount();
    const headers = [...el.querySelectorAll("thead th")].map((h) => h.textContent?.trim());
    expect(headers.some((h) => h?.startsWith("Title"))).toBe(true);
    expect(headers.some((h) => h?.startsWith("Date added"))).toBe(true);
  });

  it("shows an empty table body rather than breaking when there are no books", () => {
    const { el } = mount({ ids: [] });
    expect(el.querySelectorAll("tbody tr")).toHaveLength(0);
    expect(el.querySelectorAll("thead th").length).toBeGreaterThan(0);
  });
});

describe("sorting", () => {
  it("marks the sorted column for assistive tech and shows a direction", () => {
    const { el } = mount({ sort: "timestamp", sortOrder: "desc" });
    const sorted = [...el.querySelectorAll("thead th")].find((h) => h.getAttribute("aria-sort") !== "none");
    expect(sorted?.textContent).toContain("Date added");
    expect(sorted?.getAttribute("aria-sort")).toBe("descending");
  });

  it("emits a new sort when another column's header is clicked", () => {
    const { el, emitted } = mount({ sort: "timestamp", sortOrder: "desc" });
    const title = [...el.querySelectorAll<HTMLButtonElement>("thead button")].find((b) => b.textContent?.includes("Title"));
    title?.click();
    // Text columns start ascending.
    expect(emitted.sortBy).toEqual([{ sort: "title", order: "asc" }]);
  });

  it("flips direction when the already-sorted column is clicked", () => {
    const { el, emitted } = mount({ sort: "title", sortOrder: "asc" });
    const title = [...el.querySelectorAll<HTMLButtonElement>("thead button")].find((b) => b.textContent?.includes("Title"));
    title?.click();
    expect(emitted.sortBy).toEqual([{ sort: "title", order: "desc" }]);
  });

  it("gives an unsortable column no header button to click", () => {
    const comments: BookColumn = { key: "comments", label: "Comments", kind: "text", width: 100 };
    const { el } = mount({ columns: [comments] });
    expect(el.querySelectorAll("thead button")).toHaveLength(0);
  });
});

describe("selection", () => {
  it("opens a book when a row is clicked outside select mode", () => {
    const { el, emitted } = mount();
    el.querySelector<HTMLElement>("tbody tr")?.click();
    expect(emitted.select).toEqual([1]);
    expect(emitted.toggleSelected).toEqual([]);
  });

  it("toggles selection instead of opening while in select mode", () => {
    const { el, emitted } = mount({ selectMode: true });
    el.querySelector<HTMLElement>("tbody tr")?.click();
    expect(emitted.toggleSelected).toEqual([1]);
    expect(emitted.select).toEqual([]);
  });

  it("adds a checkbox column only in select mode", () => {
    expect(mount().el.querySelectorAll("tbody input[type=checkbox]")).toHaveLength(0);
    app?.unmount();
    host?.remove();
    expect(mount({ selectMode: true }).el.querySelectorAll("tbody input[type=checkbox]")).toHaveLength(2);
  });

  it("marks selected rows", async () => {
    const { el } = mount({ selectMode: true, selectedIds: new Set([2]) });
    await nextTick();
    const rows = el.querySelectorAll("tbody tr");
    expect(rows[0].classList.contains("selected")).toBe(false);
    expect(rows[1].classList.contains("selected")).toBe(true);
  });

  it("highlights the open book outside select mode", async () => {
    const { el } = mount({ selectedBookId: 1 });
    await nextTick();
    expect(el.querySelectorAll("tbody tr")[0].classList.contains("selected")).toBe(true);
  });

  // Rows have to stay `<tr>` for the column layout to work, so they
  // cannot be buttons -- keyboard access is supplied by hand and is
  // exactly the kind of thing that silently rots.
  it("opens a book from the keyboard", () => {
    const { el, emitted } = mount();
    const row = el.querySelector("tbody tr")!;
    row.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(emitted.select).toEqual([1]);
  });

  it("ignores keys that are not activation keys", () => {
    const { el, emitted } = mount();
    const row = el.querySelector("tbody tr")!;
    row.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    expect(emitted.select).toEqual([]);
  });
});

describe("column widths", () => {
  it("applies each column's pixel width to its header", () => {
    const wide: BookColumn = { key: "title", label: "Title", kind: "text", width: 275, sortKey: "title" };
    const { el } = mount({ columns: [wide] });
    expect(el.querySelector<HTMLElement>("thead th")?.style.width).toBe("275px");
  });

  it("gives every column a resize handle", () => {
    const { el } = mount();
    expect(el.querySelectorAll("thead .resize-handle")).toHaveLength(COLUMNS.length);
  });

  it("is exactly as wide as its columns, so a wide table scrolls instead of squeezing", () => {
    const { el } = mount();
    const sum = COLUMNS.reduce((s, c) => s + c.width, 0);
    const table = el.querySelector<HTMLElement>("table.book-table")!;
    expect(table.style.width).toBe(`${sum}px`);
    expect(el.querySelectorAll("colgroup col")).toHaveLength(COLUMNS.length);
  });
});

function pointer(type: string, x: number, y = 0) {
  return new MouseEvent(type, { clientX: x, clientY: y, bubbles: true, cancelable: true, button: 0 });
}

describe("resizing", () => {
  it("grows and shrinks live while dragging a handle, and never sorts", () => {
    const { el, emitted } = mount();
    const handle = el.querySelector<HTMLElement>("thead th .resize-handle")!;
    const start = COLUMNS[0].width;
    handle.dispatchEvent(pointer("pointerdown", 100));
    handle.dispatchEvent(pointer("pointermove", 160));
    handle.dispatchEvent(pointer("pointermove", 40));
    handle.dispatchEvent(pointer("pointerup", 40));
    expect(emitted.resize.map((r) => r.width)).toEqual([start + 60, start - 60]);
    expect(emitted.sortBy).toEqual([]);
  });

  it("stops at the minimum width", () => {
    const { el, emitted } = mount();
    const handle = el.querySelector<HTMLElement>("thead th .resize-handle")!;
    handle.dispatchEvent(pointer("pointerdown", 1000));
    handle.dispatchEvent(pointer("pointermove", 0));
    expect(emitted.resize[emitted.resize.length - 1].width).toBe(40);
  });

  it("double-click auto-fits to the header and the loaded rows", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    const longTitle = "A very very long title indeed, much longer than the default column";
    const { el, emitted } = mount({ getBook: (id: number) => ({ ...book(id), title: longTitle }) });
    el.querySelector<HTMLElement>("thead th .resize-handle")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    // jsdom has no canvas: 7px per character stands in for the measurement.
    expect(emitted.resize[emitted.resize.length - 1]).toEqual({ key: COLUMNS[0].key, width: Math.min(900, longTitle.length * 7 + 28) });
  });
});

describe("reordering columns", () => {
  function stubSpans(el: HTMLElement) {
    el.querySelectorAll<HTMLElement>("thead th[data-key]").forEach((th, i) => {
      th.getBoundingClientRect = () => ({ left: i * 100, right: i * 100 + 100, top: 0, bottom: 30, width: 100, height: 30, x: i * 100, y: 0, toJSON: () => ({}) });
    });
  }

  it("dragging a header past the threshold emits a reorder, not a sort", () => {
    const reorders: { key: string; index: number }[] = [];
    const { el, emitted } = mount({ onReorder: (v: { key: string; index: number }) => reorders.push(v) });
    stubSpans(el);
    const th = el.querySelector<HTMLElement>("thead th[data-key]")!;
    th.dispatchEvent(pointer("pointerdown", 50));
    th.dispatchEvent(pointer("pointermove", 260));
    th.dispatchEvent(pointer("pointerup", 260));
    expect(reorders).toEqual([{ key: COLUMNS[0].key, index: 3 }]);
    th.querySelector("button")!.click(); // the click a browser fires after a drag
    expect(emitted.sortBy).toEqual([]);
  });

  it("a press that barely moves is still a click that sorts", () => {
    const reorders: unknown[] = [];
    const { el, emitted } = mount({ onReorder: (v: unknown) => reorders.push(v) });
    stubSpans(el);
    const th = el.querySelector<HTMLElement>("thead th[data-key]")!;
    th.dispatchEvent(pointer("pointerdown", 50));
    th.dispatchEvent(pointer("pointermove", 52));
    th.dispatchEvent(pointer("pointerup", 52));
    th.querySelector("button")!.click();
    expect(reorders).toEqual([]);
    expect(emitted.sortBy).toHaveLength(1);
  });

  it("dropping where it already is changes nothing", () => {
    const reorders: unknown[] = [];
    const { el } = mount({ onReorder: (v: unknown) => reorders.push(v) });
    stubSpans(el);
    const th = el.querySelector<HTMLElement>("thead th[data-key]")!;
    th.dispatchEvent(pointer("pointerdown", 50));
    th.dispatchEvent(pointer("pointermove", 70));
    th.dispatchEvent(pointer("pointerup", 70));
    expect(reorders).toEqual([]);
  });
});

describe("virtual rendering", () => {
  it("renders a bounded number of rows for a 50,000 book list, with spacers for the rest", () => {
    const ids = Array.from({ length: 50_000 }, (_, i) => i + 1);
    const { el } = mount({ ids, getBook: (id: number) => book(id) });
    const rows = el.querySelectorAll("tbody tr:not(.spacer)");
    expect(rows.length).toBeLessThan(100);
    expect(rows.length).toBeGreaterThan(0);
    const bottom = el.querySelector<HTMLElement>("tbody tr.spacer");
    expect(parseInt(bottom!.style.height, 10)).toBeGreaterThan(1_000_000);
  });

  it("shows muted placeholder rows until a row has loaded", () => {
    const { el } = mount({ ids: [1, 2, 3], getBook: (id: number) => (id === 2 ? book(2) : undefined) });
    const rows = el.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(3);
    expect(rows[0].classList.contains("placeholder")).toBe(true);
    expect(rows[1].classList.contains("placeholder")).toBe(false);
    expect(rows[1].textContent).toContain("Book 2");
  });

  it("reports the rendered range so the rows can be loaded", () => {
    const ranges: { first: number; last: number }[] = [];
    mount({ ids: Array.from({ length: 5000 }, (_, i) => i + 1), onRange: (r: { first: number; last: number }) => ranges.push(r) });
    expect(ranges[0].first).toBe(0);
    expect(ranges[0].last).toBeLessThan(100);
  });

  it("moves the rendered window when the list is scrolled", async () => {
    const ids = Array.from({ length: 5000 }, (_, i) => i + 1);
    const { el } = mount({ ids, getBook: (id: number) => book(id) });
    const scroller = el.querySelector<HTMLElement>(".table-scroll")!;
    scroller.scrollTop = 30 * 2000;
    scroller.dispatchEvent(new Event("scroll"));
    await nextTick();
    const first = el.querySelector<HTMLElement>("tbody tr:not(.spacer)")!;
    expect(Number(first.dataset.id)).toBeGreaterThan(1900);
  });

  it("emits a range selection for shift-click in select mode", () => {
    const ranged: number[] = [];
    const { el } = mount({ selectMode: true, onRangeSelect: (id: number) => ranged.push(id) });
    el.querySelectorAll<HTMLElement>("tbody tr")[1].dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    expect(ranged).toEqual([2]);
  });

  it("asks to navigate on arrow and page keys", () => {
    const keys: string[] = [];
    const { el } = mount({ onNavigate: (k: string) => keys.push(k) });
    el.querySelector("tbody tr")!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    el.querySelector("tbody tr")!.dispatchEvent(new KeyboardEvent("keydown", { key: "PageDown", bubbles: true }));
    expect(keys).toEqual(["ArrowDown", "PageDown"]);
  });
});

describe("editing in place", () => {
  const EDIT_COLUMNS: BookColumn[] = resolveColumns(columnsFor({}, [["title", "Title"], ["authors", "Authors"], ["series", "Series"]]), { ...DEFAULT_TABLE_PREFS, columns: ["title", "authors", "series", "formats"] });

  /** A table wired to a tiny reactive store, with `save` standing in for the set-fields call. */
  function mountEditable(rows: BookSummary[], save: (id: number, changes: BookFieldChanges) => Promise<BookSummary>, extra: Record<string, unknown> = {}) {
    const store = shallowReactive(new Map(rows.map((b) => [b.id, b])));
    const target = {
      getById: (id: number) => store.get(id),
      patch: (id: number, c: Partial<BookSummary>) => {
        const before = store.get(id);
        if (before) store.set(id, { ...before, ...c } as BookSummary);
        return before;
      },
      put: (b: BookSummary) => void store.set(b.id, b),
    };
    return {
      store,
      ...mount({
        columns: EDIT_COLUMNS,
        ids: rows.map((r) => r.id),
        getBook: (id: number) => store.get(id),
        commit: (bookId: number, column: BookColumn, raw: string) => commitCellEdit({ target, bookId, column, raw, save }),
        ...extra,
      }),
    };
  }

  const cell = (el: HTMLElement, row: number, col: number) => el.querySelectorAll("tbody tr")[row].querySelectorAll("td")[col] as HTMLElement;
  const flush = async () => {
    await nextTick();
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  };
  const type = (el: HTMLElement, text: string, key: string) => {
    const input = el.querySelector<HTMLInputElement>("input.cell-editor")!;
    input.value = text;
    input.dispatchEvent(new Event("input"));
    input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
  };

  it("a single click in an empty cell only selects the row; double-click opens the input, and Enter saves through set-fields", async () => {
    const calls: [number, BookFieldChanges][] = [];
    const { el, store } = mountEditable([book(1, { title: "" })], async (id, changes) => {
      calls.push([id, changes]);
      return book(id, { title: String(changes.title) });
    });
    expect(cell(el, 0, 0).classList.contains("empty-editable")).toBe(true);
    expect(cell(el, 0, 0).getAttribute("data-hint")).toBe("Double-click to set title…");

    cell(el, 0, 0).click();
    await nextTick();
    expect(el.querySelector("input.cell-editor")).toBeNull();

    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    expect(el.querySelector("input.cell-editor")).not.toBeNull();

    type(el, "My New Title", "Enter");
    await flush();

    expect(calls).toEqual([[1, { title: "My New Title" }]]);
    expect(el.querySelector("input.cell-editor")).toBeNull();
    expect(cell(el, 0, 0).textContent?.trim()).toBe("My New Title");
    expect(store.get(1)!.title).toBe("My New Title");
  });

  it("a single click on a filled cell only selects the row", async () => {
    const { el, emitted } = mountEditable([book(1)], async () => book(1));
    cell(el, 0, 0).click();
    await nextTick();
    expect(el.querySelector("input.cell-editor")).toBeNull();
    expect(emitted.select).toEqual([1]);
  });

  it("double-click edits a filled cell, with its value in the input", async () => {
    const { el } = mountEditable([book(1, { title: "Old" })], async () => book(1));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    expect(el.querySelector<HTMLInputElement>("input.cell-editor")!.value).toBe("Old");
  });

  it("F2 edits the title of the selected row", async () => {
    const { el } = mountEditable([book(1, { title: "Old" })], async () => book(1), { selectedBookId: 1 });
    // Vue ignores an event fired in the same millisecond its listener was attached.
    await new Promise((r) => setTimeout(r, 5));
    el.querySelector("tbody tr")!.dispatchEvent(new KeyboardEvent("keydown", { key: "F2", bubbles: true }));
    await nextTick();
    expect(el.querySelector<HTMLInputElement>("input.cell-editor")!.value).toBe("Old");
  });

  it("Escape cancels without saving", async () => {
    let calls = 0;
    const { el, store } = mountEditable([book(1, { title: "Old" })], async () => ((calls += 1), book(1)));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    type(el, "Nope", "Escape");
    await flush();
    expect(el.querySelector("input.cell-editor")).toBeNull();
    expect(calls).toBe(0);
    expect(store.get(1)!.title).toBe("Old");
  });

  it("Tab saves and moves to the next editable cell", async () => {
    const calls: BookFieldChanges[] = [];
    const { el } = mountEditable([book(1, { title: "Old" })], async (id, changes) => (calls.push(changes), book(id, { title: String(changes.title ?? "Old") })));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    type(el, "Tabbed", "Tab");
    await flush();
    expect(calls).toEqual([{ title: "Tabbed" }]);
    expect(cell(el, 0, 1).querySelector("input.cell-editor")).not.toBeNull();
  });

  it("blur with an unchanged value closes the editor without calling the API", async () => {
    let calls = 0;
    const { el } = mountEditable([book(1, { title: "Same" })], async () => ((calls += 1), book(1)));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    el.querySelector<HTMLInputElement>("input.cell-editor")!.dispatchEvent(new Event("blur"));
    await flush();
    expect(calls).toBe(0);
    expect(el.querySelector("input.cell-editor")).toBeNull();
  });

  it("blur saves a changed value", async () => {
    const calls: BookFieldChanges[] = [];
    const { el } = mountEditable([book(1, { title: "Old" })], async (id, changes) => (calls.push(changes), book(id, { title: String(changes.title) })));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    const input = el.querySelector<HTMLInputElement>("input.cell-editor")!;
    input.value = "Blurred";
    input.dispatchEvent(new Event("input"));
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(calls).toEqual([{ title: "Blurred" }]);
  });

  it("on a failed save rolls the row back, keeps the editor open with the typed value and shows the error", async () => {
    const { el, store } = mountEditable([book(1, { title: "Old" })], async () => {
      throw new Error("read-only library");
    });
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    type(el, "Typed", "Enter");
    await flush();
    expect(store.get(1)!.title).toBe("Old");
    expect(el.querySelector<HTMLInputElement>("input.cell-editor")!.value).toBe("Typed");
    expect(el.querySelector(".cell-error")?.textContent).toContain("read-only library");
  });

  it("a parse error stays inline too", async () => {
    const { el } = mountEditable([book(1, { title: "Old" })], async () => book(1));
    cell(el, 0, 0).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    type(el, "   ", "Enter");
    await flush();
    expect(el.querySelector(".cell-error")?.textContent).toContain("empty");
  });

  it("read-only columns never open an editor", async () => {
    const { el } = mountEditable([book(1)], async () => book(1));
    cell(el, 0, 3).dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    cell(el, 0, 3).click();
    await nextTick();
    expect(el.querySelector(".cell-editor")).toBeNull();
  });

  it("is read-only when no commit handler is given", async () => {
    const { el } = mount({ columns: EDIT_COLUMNS, ids: [1], getBook: () => book(1, { title: "" }) });
    cell(el, 0, 0).click();
    await nextTick();
    expect(el.querySelector(".cell-editor")).toBeNull();
  });

  it("does not start editing from a click in select mode", async () => {
    const { el } = mountEditable([book(1, { title: "" })], async () => book(1), { selectMode: true });
    el.querySelectorAll<HTMLElement>("tbody tr")[0].querySelectorAll<HTMLElement>("td")[1].click();
    await nextTick();
    expect(el.querySelector(".cell-editor")).toBeNull();
  });
});

describe("suggestions while editing in place", () => {
  const COLS: BookColumn[] = resolveColumns(columnsFor({}, [["title", "Title"], ["authors", "Authors"]]), { ...DEFAULT_TABLE_PREFS, columns: ["title", "authors"] });
  const flush = async () => {
    for (let i = 0; i < 4; i++) await Promise.resolve();
    await nextTick();
    await nextTick();
    await new Promise((r) => setTimeout(r, 0));
  };

  async function open(save: (id: number, changes: BookFieldChanges) => Promise<BookSummary>) {
    invalidateFieldValues();
    await getFieldValues("authors", async () => [{ name: "Jane Doe", count: 4 }, { name: "John Smith", count: 2 }]);
    const store = new Map([[1, book(1, { authors: ["Ann Lee"] })]]);
    const target = { getById: (id: number) => store.get(id), patch: (id: number) => store.get(id), put: (b: BookSummary) => void store.set(b.id, b) };
    const m = mount({ columns: COLS, ids: [1], getBook: (id: number) => store.get(id), commit: (bookId: number, column: BookColumn, raw: string) => commitCellEdit({ target, bookId, column, raw, save }) });
    const cell = m.el.querySelectorAll("tbody tr")[0].querySelectorAll("td")[1] as HTMLElement;
    cell.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await nextTick();
    const input = m.el.querySelector<HTMLInputElement>("input.cell-editor")!;
    input.value = "Ann Lee & jo";
    input.dispatchEvent(new Event("input"));
    await flush();
    return { ...m, input };
  }
  const press = (input: HTMLInputElement, key: string) => input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

  it("accepting a suggestion with Down+Enter does not save; the next Enter does", async () => {
    const calls: BookFieldChanges[] = [];
    const { input } = await open(async (id, changes) => (calls.push(changes), book(id, { authors: changes.authors })));
    expect(document.body.querySelectorAll('[role="option"]').length).toBe(1);
    press(input, "ArrowDown");
    await flush();
    press(input, "Enter");
    await flush();
    expect(calls).toEqual([]);
    expect(input.value).toBe("Ann Lee & John Smith & ");
    press(input, "Enter");
    await flush();
    expect(calls).toEqual([{ authors: ["Ann Lee", "John Smith"] }]);
  });

  it("Enter with nothing highlighted saves what was typed", async () => {
    const calls: BookFieldChanges[] = [];
    const { input } = await open(async (id, changes) => (calls.push(changes), book(id, { authors: changes.authors })));
    press(input, "Enter");
    await flush();
    expect(calls).toEqual([{ authors: ["Ann Lee", "jo"] }]);
  });

  it("the first Esc closes the list and the second cancels the edit", async () => {
    const calls: BookFieldChanges[] = [];
    const { el, input } = await open(async (id, changes) => (calls.push(changes), book(id)));
    press(input, "Escape");
    await flush();
    expect(el.querySelector("input.cell-editor")).not.toBeNull();
    press(input, "Escape");
    await flush();
    expect(el.querySelector("input.cell-editor")).toBeNull();
    expect(calls).toEqual([]);
  });
});

describe("multi-select without select mode", () => {
  const row = (el: HTMLElement, i: number) => el.querySelectorAll("tbody tr")[i] as HTMLElement;
  const click = (el: HTMLElement, i: number, init: MouseEventInit = {}) => row(el, i).dispatchEvent(new MouseEvent("click", { bubbles: true, ...init }));

  it("a plain click selects, Ctrl/Cmd-click toggles and Shift-click selects a range, outside select mode", () => {
    const ranged: number[] = [];
    const { el, emitted } = mount({ onRangeSelect: (id: number) => ranged.push(id) });
    click(el, 0);
    click(el, 1, { ctrlKey: true });
    click(el, 0, { metaKey: true });
    click(el, 1, { shiftKey: true });
    expect(emitted.select).toEqual([1]);
    expect(emitted.toggleSelected).toEqual([2, 1]);
    expect(ranged).toEqual([2]);
  });

  it("Ctrl+A asks to select everything, and Shift+arrow asks to extend", () => {
    let all = 0;
    const nav: [string, boolean][] = [];
    const { el } = mount({ onSelectAll: () => all++, onNavigate: (k: string, extend: boolean) => nav.push([k, extend]) });
    const scroller = el.querySelector(".table-scroll")!;
    const prevented = new KeyboardEvent("keydown", { key: "a", ctrlKey: true, bubbles: true, cancelable: true });
    scroller.dispatchEvent(prevented);
    expect(all).toBe(1);
    expect(prevented.defaultPrevented).toBe(true);
    scroller.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", shiftKey: true, bubbles: true, cancelable: true }));
    scroller.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    expect(nav).toEqual([["ArrowDown", true], ["ArrowDown", false]]);
  });

  it("marks every ctrl-selected row as selected", () => {
    const { el } = mount({ selectedIds: new Set([1, 2]), selectedBookId: 2 });
    expect(el.querySelectorAll("tbody tr.selected")).toHaveLength(2);
  });
});

describe("keyboard and screen reader access", () => {
  const rowsOf = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("tbody tr[data-id]")];

  it("is a grid with the book count, and every row knows its place", () => {
    const { el } = mount({ ids: [1, 2, 3], getBook: (id: number) => book(id) });
    const table = el.querySelector("table")!;
    expect(table.getAttribute("role")).toBe("grid");
    expect(table.getAttribute("aria-rowcount")).toBe("3");
    expect(rowsOf(el).map((r) => r.getAttribute("aria-rowindex"))).toEqual(["1", "2", "3"]);
  });

  it("reports the whole list's size, not just the rendered rows", () => {
    const ids = Array.from({ length: 50000 }, (_, i) => i + 1);
    const { el } = mount({ ids, getBook: (id: number) => book(id) });
    expect(el.querySelector("table")!.getAttribute("aria-rowcount")).toBe("50000");
    expect(rowsOf(el).length).toBeLessThan(200);
  });

  it("marks selected rows with aria-selected", () => {
    const { el } = mount({ selectedIds: new Set([2]), selectMode: true });
    const [first, second] = rowsOf(el);
    expect(first.getAttribute("aria-selected")).toBe("false");
    expect(second.getAttribute("aria-selected")).toBe("true");
  });

  it("puts exactly one row in the Tab order: the current one", () => {
    const { el } = mount({ ids: [1, 2, 3], getBook: (id: number) => book(id), selectedBookId: 2 });
    expect(rowsOf(el).filter((r) => r.tabIndex === 0).map((r) => r.dataset.id)).toEqual(["2"]);
    expect(rowsOf(el).filter((r) => r.getAttribute("tabindex") === "-1")).toHaveLength(2);
  });

  it("falls back to the first loaded row when nothing is selected", () => {
    const { el } = mount({ ids: [1, 2, 3], getBook: (id: number) => (id === 1 ? undefined : book(id)), selectedBookId: null });
    expect(rowsOf(el).filter((r) => r.tabIndex === 0).map((r) => r.dataset.id)).toEqual(["2"]);
  });

  it("moves the Tab stop, and focus with it, when the current row changes", async () => {
    const selected = shallowReactive({ id: 1 as number | null });
    host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ render: () => h(BookTable, { ids: [1, 2, 3], getBook: (id: number) => book(id), columns: COLUMNS, selectMode: false, selectedIds: new Set<number>(), selectedBookId: selected.id, sort: "timestamp", sortOrder: "desc" as const }) });
    app.mount(host);
    const rows = () => rowsOf(host!);
    rows()[0].focus();
    expect(document.activeElement).toBe(rows()[0]);
    selected.id = 2;
    for (let i = 0; i < 4; i++) await nextTick();
    expect(rows().filter((r) => r.tabIndex === 0).map((r) => r.dataset.id)).toEqual(["2"]);
    expect(document.activeElement).toBe(rows()[1]);
  });

  it("gives every header an aria-sort", () => {
    const { el } = mount();
    for (const th of el.querySelectorAll("thead th")) expect(["ascending", "descending", "none"]).toContain(th.getAttribute("aria-sort"));
  });

  it("makes each resize handle a focusable separator that reports its width", () => {
    const { el } = mount();
    const handles = [...el.querySelectorAll<HTMLElement>('[role="separator"]')];
    expect(handles).toHaveLength(COLUMNS.length);
    handles.forEach((handle, i) => {
      expect(handle.tabIndex).toBe(0);
      expect(handle.getAttribute("aria-valuenow")).toBe(String(COLUMNS[i].width));
    });
  });

  it("resizes with Left and Right, and in bigger steps with Shift", () => {
    const { el, emitted } = mount();
    const handle = el.querySelector<HTMLElement>('[role="separator"]')!;
    const width = COLUMNS[0].width;
    const press = (key: string, shiftKey = false) => handle.dispatchEvent(new KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true }));
    press("ArrowRight");
    press("ArrowLeft");
    press("ArrowRight", true);
    press("ArrowLeft", true);
    expect(emitted.resize).toEqual([
      { key: COLUMNS[0].key, width: width + 10 },
      { key: COLUMNS[0].key, width: width - 10 },
      { key: COLUMNS[0].key, width: width + 50 },
      { key: COLUMNS[0].key, width: width - 50 },
    ]);
    // Other keys leave the width alone.
    press("ArrowUp");
    expect(emitted.resize).toHaveLength(4);
  });

  it("does not let a resize key reach the list's key handling", () => {
    const { el, emitted } = mount();
    const handle = el.querySelector<HTMLElement>('[role="separator"]')!;
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    expect(emitted.select).toEqual([]);
  });
});
