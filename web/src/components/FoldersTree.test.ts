// The Folders tree draws only the rows inside its scroll window (a share can
// have thousands of folders), and still expands, collapses, takes a dropped
// book and opens its context menu on the rows it does draw.

import { createApp, nextTick, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import FoldersTree from "./FoldersTree.vue";
import { BOOKS_DRAG_TYPE } from "../library/folders";
import type { LibraryFolder } from "../library/api";

const api = vi.hoisted(() => ({ folders: [] as unknown[], planMoveBooks: vi.fn() }));

vi.mock("../library/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../library/api")>()),
  fetchLibraryFolders: async () => ({ folders: api.folders, total: api.folders.length }),
  planMoveBooks: api.planMoveBooks,
}));

let app: App | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  localStorage.clear();
  api.planMoveBooks.mockReset();
  api.planMoveBooks.mockResolvedValue({ books: 1, files: 1, already_organized: 0, conflicts: 0, skipped: [], moves: [{ from: "a", to: "b" }], plan_fingerprint: "x" });
});

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

function folder(path: string, books = 1): LibraryFolder {
  return { path, books, books_here: books, empty: books === 0 } as LibraryFolder;
}

async function mount(folders: LibraryFolder[]) {
  api.folders = folders;
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(FoldersTree, { selectedIds: [7], refreshKey: 0 });
  app.mount(host);
  await settle();
}

async function settle() {
  for (let i = 0; i < 5; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

const rowEls = () => Array.from(host!.querySelectorAll<HTMLElement>(".folder-row"));
const rowPaths = () => rowEls().map((r) => r.dataset.folder);

function dragEvent(type: string, data: Record<string, string>): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: { types: Object.keys(data), getData: (t: string) => data[t] ?? "", dropEffect: "none" } });
  return event;
}

describe("the Folders tree", () => {
  it("draws only the window of rows when there are 5,000 folders", async () => {
    await mount(Array.from({ length: 5000 }, (_, i) => folder(`f${String(i).padStart(4, "0")}`)));
    const drawn = rowEls().length;
    expect(drawn).toBeGreaterThan(0);
    expect(drawn).toBeLessThan(80);
    expect(rowPaths()[0]).toBe("f0000");
    // The scrollbar still stands for all of them: spacers carry the rest of the height.
    const list = host!.querySelector<HTMLElement>("[data-testid=folder-list]")!;
    const spacers = Array.from(list.querySelectorAll<HTMLElement>("li[aria-hidden=true]"));
    const spacerHeight = spacers.reduce((sum, li) => sum + parseFloat(li.style.height), 0);
    expect(spacerHeight + drawn * 22).toBe(5000 * 22);
  });

  it("moves the window when the section scrolls", async () => {
    await mount(Array.from({ length: 5000 }, (_, i) => folder(`f${String(i).padStart(4, "0")}`)));
    const root = host!.querySelector<HTMLElement>("section.folders")!;
    // jsdom does no layout, so the scroll position is stated.
    const scrolled = 22 * 2000;
    Object.defineProperty(root, "scrollTop", { value: scrolled, configurable: true });
    // The header is 22px tall, so the list starts 22px below the section's top edge, less what was scrolled.
    const list = host!.querySelector<HTMLElement>("[data-testid=folder-list]")!;
    list.getBoundingClientRect = () => ({ top: 22 - scrolled }) as DOMRect;
    root.dispatchEvent(new Event("scroll"));
    await settle();
    const paths = rowPaths();
    expect(paths).not.toContain("f0000");
    expect(paths).toContain("f2000");
    expect(paths.length).toBeLessThan(80);
  });

  it("expands and collapses a folder, changing the rows", async () => {
    await mount([folder("a", 2), folder("a/b"), folder("a/c"), folder("z")]);
    expect(rowPaths()).toEqual(["a", "z"]);
    const twisty = () => rowEls()[0].querySelector<HTMLButtonElement>(".twisty-btn")!;
    expect(twisty().getAttribute("aria-label")).toBe("Expand");
    twisty().click();
    await settle();
    expect(rowPaths()).toEqual(["a", "a/b", "a/c", "z"]);
    expect(twisty().getAttribute("aria-label")).toBe("Collapse");
    twisty().click();
    await settle();
    expect(rowPaths()).toEqual(["a", "z"]);
  });

  it("highlights the folder that was picked and reports its search", async () => {
    await mount([folder("a"), folder("b")]);
    const emitted: unknown[][] = [];
    app!.unmount();
    host!.remove();
    host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp(FoldersTree, { selectedIds: [], refreshKey: 0, onSelect: (...args: unknown[]) => emitted.push(args) });
    app.mount(host);
    await settle();
    rowEls()[1].querySelector<HTMLButtonElement>(".folder-pick")!.click();
    await settle();
    expect(rowEls()[1].classList.contains("active")).toBe(true);
    expect(rowEls()[0].classList.contains("active")).toBe(false);
    expect(emitted).toHaveLength(1);
    expect(emitted[0][1]).toBe("b");
  });

  it("still takes a book dropped on a drawn row", async () => {
    await mount(Array.from({ length: 500 }, (_, i) => folder(`f${String(i).padStart(3, "0")}`)));
    const target = rowEls()[3];
    const over = dragEvent("dragover", { [BOOKS_DRAG_TYPE]: "[7]" });
    target.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    await settle();
    expect(rowEls()[3].classList.contains("drop")).toBe(true);
    target.dispatchEvent(dragEvent("drop", { [BOOKS_DRAG_TYPE]: "[7]" }));
    await settle();
    expect(api.planMoveBooks).toHaveBeenCalledWith([7], "f003");
    expect(rowEls()[3].classList.contains("drop")).toBe(false);
  });

  it("ignores a drag that does not carry books", async () => {
    await mount([folder("a")]);
    const over = dragEvent("dragover", { "text/plain": "x" });
    rowEls()[0].dispatchEvent(over);
    expect(over.defaultPrevented).toBe(false);
  });

  it("opens the context menu on a drawn row", async () => {
    await mount(Array.from({ length: 500 }, (_, i) => folder(`f${String(i).padStart(3, "0")}`)));
    rowEls()[2].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 20 }));
    await settle();
    const menu = document.body.querySelector("[role=menu]");
    expect(menu).not.toBeNull();
    expect(menu!.textContent).toContain("Rename folder");
    expect(menu!.textContent).toContain("Move 1 selected book here");
  });
});

describe("the Folders tree as an ARIA tree", () => {
  const key = async (el: HTMLElement, k: string) => {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
    await settle();
  };
  const tabStops = () => Array.from(host!.querySelectorAll<HTMLElement>("[data-folder],button")).filter((e) => e.tabIndex >= 0);

  it("is a tree of treeitems with level and expanded state", async () => {
    await mount([folder("a", 2), folder("a/b"), folder("z")]);
    const tree = host!.querySelector("[role=tree]");
    expect(tree).not.toBeNull();
    const items = Array.from(tree!.querySelectorAll<HTMLElement>("[role=treeitem]"));
    expect(items.map((i) => i.dataset.folder)).toEqual(["a", "z"]);
    expect(items[0].getAttribute("aria-level")).toBe("1");
    expect(items[0].getAttribute("aria-expanded")).toBe("false");
    expect(items[1].hasAttribute("aria-expanded")).toBe(false);
    await key(items[0], "ArrowRight");
    const after = Array.from(tree!.querySelectorAll<HTMLElement>("[role=treeitem]"));
    expect(after.map((i) => i.dataset.folder)).toEqual(["a", "a/b", "z"]);
    expect(after[0].getAttribute("aria-expanded")).toBe("true");
    expect(after[1].getAttribute("aria-level")).toBe("2");
  });

  it("is one Tab stop, however many folders there are", async () => {
    await mount(Array.from({ length: 300 }, (_, i) => folder(`f${String(i).padStart(3, "0")}`)));
    const rows = rowEls();
    expect(rows.length).toBeGreaterThan(1);
    expect(rows.filter((r) => r.tabIndex === 0)).toHaveLength(1);
    // The header buttons are the only other stops; no folder button is one.
    expect(tabStops().filter((e) => e.matches(".folder-pick,.twisty-btn"))).toHaveLength(0);
  });

  it("moves with the arrows, expands with right, collapses and climbs with left, and picks with Enter", async () => {
    const emitted: unknown[][] = [];
    api.folders = [folder("a", 2), folder("a/b"), folder("z")];
    host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp(FoldersTree, { selectedIds: [], refreshKey: 0, onSelect: (...args: unknown[]) => emitted.push(args) });
    app.mount(host);
    await settle();

    rowEls()[0].focus();
    await key(rowEls()[0], "ArrowRight");
    expect(rowPaths()).toEqual(["a", "a/b", "z"]);
    await key(rowEls()[0], "ArrowRight");
    expect(document.activeElement).toBe(rowEls()[1]);
    await key(rowEls()[1], "ArrowDown");
    expect(document.activeElement).toBe(rowEls()[2]);
    expect(rowEls()[2].tabIndex).toBe(0);
    expect(rowEls()[0].tabIndex).toBe(-1);
    await key(rowEls()[2], "ArrowUp");
    await key(rowEls()[1], "ArrowLeft");
    expect(document.activeElement).toBe(rowEls()[0]);
    await key(rowEls()[0], "ArrowLeft");
    expect(rowPaths()).toEqual(["a", "z"]);
    await key(rowEls()[1], "Enter");
    expect(emitted).toHaveLength(1);
    expect(emitted[0][1]).toBe("z");
    expect(rowEls()[1].getAttribute("aria-selected")).toBe("true");
  });
});
