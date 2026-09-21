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

import { createApp, nextTick, type App } from "vue";
import { afterEach, describe, expect, it } from "vitest";

import BookTable from "./BookTable.vue";
import { columnsFor, resolveColumns, DEFAULT_TABLE_PREFS, type BookColumn } from "../library/columns";
import type { BookSummary } from "../library/types";

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
    books: [book(1), book(2)],
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
    const { el } = mount({ books: [] });
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
});
