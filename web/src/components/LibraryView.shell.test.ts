// The library window's shell, mounted with the network mocked: the toolbar's menus, the
// keyboard, the side panels in a narrow window, the empty and offline states and the
// status bar. (Selection, delete, keep-alive and listener hygiene are in LibraryView.test.ts.)

import { createApp, defineComponent, h, nextTick, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BookSummary } from "../library/types";

const state = vi.hoisted(() => ({ tauri: false }));
const apiMocks = vi.hoisted(() => ({
  search: vi.fn(),
  fetchBooks: vi.fn(),
  fetchFieldMetadata: vi.fn(),
  fetchVirtualLibraries: vi.fn(),
  fetchSavedSearches: vi.fn(),
  fetchLibraryInfo: vi.fn(),
  fetchLibraryJobs: vi.fn(),
  fetchUntracked: vi.fn(),
  fetchTrash: vi.fn(),
  setFields: vi.fn(),
}));
const settingsMocks = vi.hoisted(() => ({ fetchProfile: vi.fn(async (_name: string): Promise<unknown> => null), saveProfile: vi.fn(async (_name: string, _profile: unknown): Promise<void> => undefined) }));

vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), ...apiMocks }));
vi.mock("../settings/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../settings/api")>()), ...settingsMocks }));
vi.mock("../tauri", () => ({ isTauri: () => state.tauri, tauriInvoke: vi.fn(async (cmd: string) => (cmd === "list_recent_libraries" ? [] : undefined)) }));
vi.mock("./BookDetailsPanel.vue", () => ({
  default: defineComponent({
    props: ["bookId", "pendingAction"],
    setup: (p) => () => h("div", { "data-testid": "details-stub", "data-pending": p.pendingAction?.id ?? "" }, `details ${p.bookId}`),
  }),
}));
vi.mock("./CategoryBrowser.vue", () => ({ default: defineComponent({ setup: () => () => h("nav", { "data-testid": "categories-stub" }) }) }));

import LibraryView from "./LibraryView.vue";
import { UNREACHABLE } from "../library/errorText";

const makeBook = (id: number): BookSummary =>
  ({
    id,
    title: `Book ${id}`,
    authors: ["Ann Lee"],
    series: null,
    series_index: 1,
    tags: [],
    rating: null,
    publisher: "Acme",
    pubdate: null,
    timestamp: "2026-09-21 12:20:34",
    last_modified: null,
    cover: `/get/cover/${id}`,
    thumbnail: `/get/thumb/${id}`,
    formats: ["pdf"],
    main_format: { pdf: `/get/pdf/${id}` },
    other_formats: {},
  }) as BookSummary;

let results: Record<string, number[]> = {};
const RouterLinkStub = defineComponent({ setup: (_p, { slots }) => () => h("a", slots.default?.()) });

let app: App | null = null;
let host: HTMLElement | null = null;

async function settle() {
  for (let i = 0; i < 8; i++) {
    await Promise.resolve();
    await nextTick();
  }
  await new Promise((r) => setTimeout(r, 0));
  await nextTick();
}

async function mountView() {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(LibraryView);
  app.component("router-link", RouterLinkStub);
  app.mount(host);
  await settle();
  return host;
}

beforeEach(() => {
  window.innerWidth = 1400;
  localStorage.clear();
  state.tauri = false;
  results = { "": [1, 2, 3, 4, 5, 6], "title:one": [1] };
  apiMocks.search.mockImplementation(async (p: { query: string }) => ({ book_ids: results[p.query] ?? [], total_num: (results[p.query] ?? []).length }));
  apiMocks.fetchBooks.mockImplementation(async (ids: number[]) => ids.map(makeBook));
  apiMocks.fetchFieldMetadata.mockResolvedValue({ field_metadata: {}, sortable_fields: [["title", "Title"], ["timestamp", "Date added"]] });
  apiMocks.fetchVirtualLibraries.mockResolvedValue({});
  apiMocks.fetchSavedSearches.mockResolvedValue({});
  apiMocks.fetchLibraryInfo.mockResolvedValue({ library_name: "scratch", library_path: "C:/scratch" });
  apiMocks.fetchLibraryJobs.mockResolvedValue([]);
  apiMocks.fetchUntracked.mockResolvedValue({ untracked: 0, capped: false });
  apiMocks.fetchTrash.mockResolvedValue({ count: 0, total_size: 0, expire_after_seconds: 0, expired: [], entries: [] });
  settingsMocks.fetchProfile.mockResolvedValue(null);
  settingsMocks.saveProfile.mockClear();
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404, statusText: "Not found", headers: new Headers(), text: async () => "" })));
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  vi.spyOn(console, "warn").mockImplementation(() => undefined);
});

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("tbody tr[data-id]:not(.placeholder)")];
const click = (node: HTMLElement, init: MouseEventInit = {}) => node.dispatchEvent(new MouseEvent("click", { bubbles: true, ...init }));
const key = (k: string, init: KeyboardEventInit = {}, target: EventTarget = window) => target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
const statusText = (el: HTMLElement) => el.querySelector("footer")?.textContent ?? "";
const slot = (el: HTMLElement, label: string) => [...el.querySelectorAll<HTMLElement>(".tb-button")].find((b) => b.querySelector(".tb-label")?.textContent?.trim() === label)!;
const menuItems = () => [...document.querySelectorAll<HTMLElement>(".context-menu [role^='menuitem']")].map((i) => i.textContent?.replace(/\s+/g, " ").trim());

async function search(el: HTMLElement, text: string) {
  const input = el.querySelector<HTMLInputElement>("form.search input[type='search']")!;
  input.value = text;
  input.dispatchEvent(new Event("input"));
  el.querySelector<HTMLFormElement>("form.search")!.dispatchEvent(new Event("submit", { cancelable: true }));
  await settle();
}

describe("toolbar menus", () => {
  it("opens a slot's menu under that button, not at the window's corner", async () => {
    const el = await mountView();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ left: 321, bottom: 77, top: 40, right: 400, width: 0, height: 0, x: 321, y: 40, toJSON: () => ({}) } as DOMRect);
    click(slot(el, "More").querySelector<HTMLElement>(".tb-body")!);
    await settle();
    const menu = document.querySelector<HTMLElement>(".context-menu")!;
    expect(menu).not.toBeNull();
    expect(menu.style.left).toBe("321px");
    expect(menu.style.top).toBe("77px");
  });

  it("opens the library menu and the Sort menu under their buttons too", async () => {
    const el = await mountView();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ left: 150, bottom: 90, top: 60, right: 200, width: 0, height: 0, x: 150, y: 60, toJSON: () => ({}) } as DOMRect);
    click(slot(el, "Library").querySelector<HTMLElement>(".tb-body")!);
    await settle();
    expect(document.querySelector<HTMLElement>(".context-menu")!.style.left).toBe("150px");
    // A click elsewhere closes it, then the Sort button.
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await settle();
    click(el.querySelector<HTMLElement>(".tb-body[title^=\"Sorted by\"]")!);
    await settle();
    const menu = document.querySelector<HTMLElement>(".context-menu")!;
    expect(menu.style.left).toBe("150px");
    expect(menu.style.top).toBe("90px");
  });

  it("gives the Move to Trash arrow a real menu with Open Trash in it, and Help no dead arrow", async () => {
    const el = await mountView();
    const remove = slot(el, "Move to Trash");
    expect(remove).toBeDefined();
    click(remove.querySelector<HTMLElement>(".tb-chevron")!);
    await settle();
    expect(menuItems()).toEqual(["Open Trash…"]);
    expect(slot(el, "Help…").querySelector(".tb-chevron")).toBeNull();
  });

  it("names every arrow, says it opens a menu, and reports whether it is open", async () => {
    const el = await mountView();
    const arrows = [...el.querySelectorAll<HTMLElement>(".tb-chevron")];
    expect(arrows.length).toBeGreaterThan(4);
    for (const a of arrows) {
      const name = a.getAttribute("aria-label")!;
      expect(name.trim(), "an arrow with no name").not.toBe("");
      expect(name, `"${name}" has a doubled space`).not.toMatch(/\s{2}/);
      expect(name).not.toMatch(/^More More/);
      expect(a.getAttribute("aria-haspopup")).toBe("menu");
      expect(a.getAttribute("aria-expanded")).toBe("false");
    }
    const names = arrows.map((a) => a.getAttribute("aria-label"));
    expect(names).toContain("Virtual library options");
    expect(names).toContain("Saved searches options");
    expect(names).toContain("More actions");
    const first = el.querySelector<HTMLElement>(".toolbar-actions .tb-chevron")!;
    click(first);
    await settle();
    expect(first.getAttribute("aria-expanded")).toBe("true");
  });
});

describe("shortcuts", () => {
  it("shows the key that is really bound in each tooltip, and no Ctrl chord nothing handles", async () => {
    const el = await mountView();
    const title = (label: string) => slot(el, label).querySelector<HTMLElement>(".tb-body")!.title;
    expect(title("Find duplicates…")).toContain("(d)");
    expect(title("Edit metadata")).toContain("(e)");
    expect(title("Add Books…")).not.toContain("Ctrl");
    expect(title("Add Books…")).toContain("(a)");
  });

  it("shows a rebound key, not the one it shipped with", async () => {
    settingsMocks.fetchProfile.mockImplementation(async (name: string) => (name === "keymap" ? { "find-duplicates": "g" } : null));
    const el = await mountView();
    expect(slot(el, "Find duplicates…").querySelector<HTMLElement>(".tb-body")!.title).toContain("(g)");
  });

  it("Ctrl+A is not Add books, and Ctrl+A in the table selects every book", async () => {
    const picker = vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(() => undefined);
    const el = await mountView();
    key("a", { ctrlKey: true });
    await settle();
    expect(picker).not.toHaveBeenCalled();
    click(rows(el)[0]);
    await settle();
    key("a", { ctrlKey: true }, rows(el)[0]);
    await settle();
    expect(statusText(el)).toContain("6 selected");
  });

  it("Enter on the current row opens that book in the reader (the details panel performs it)", async () => {
    const el = await mountView();
    click(rows(el)[2]);
    await settle();
    key("Enter", {}, rows(el)[2]);
    await settle();
    expect(el.querySelector("[data-testid='details-stub']")!.getAttribute("data-pending")).toBe("read");
  });

  it("Enter does not read while a button has the focus, nor while a cell is being edited", async () => {
    const el = await mountView();
    click(rows(el)[2]);
    await settle();
    key("Enter", {}, el.querySelector<HTMLElement>(".tb-body[title^=\"Sorted by\"]")!);
    await settle();
    expect(el.querySelector("[data-testid='details-stub']")!.getAttribute("data-pending")).toBe("");
    key("Enter", {}, el.querySelector<HTMLInputElement>("form.search input")!);
    await settle();
    expect(el.querySelector("[data-testid='details-stub']")!.getAttribute("data-pending")).toBe("");
  });

  it("e edits one book in the panel, and opens bulk edit when several are selected", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    await settle();
    key("e");
    await settle();
    expect(el.querySelector("[data-testid='details-stub']")!.getAttribute("data-pending")).toBe("edit-metadata");
    expect(el.querySelector(".bulk-panel")).toBeNull();
    click(rows(el)[1], { ctrlKey: true });
    await settle();
    key("e");
    await settle();
    expect(el.querySelector(".bulk-panel")).not.toBeNull();
  });

  it("the main Edit metadata button is on and says how many when several books are selected", async () => {
    const el = await mountView();
    expect(slot(el, "Edit metadata").classList.contains("disabled")).toBe(true);
    click(rows(el)[0]);
    click(rows(el)[1], { ctrlKey: true });
    click(rows(el)[2], { ctrlKey: true });
    await settle();
    const button = slot(el, "Edit metadata (3)");
    expect(button.classList.contains("disabled")).toBe(false);
    click(button.querySelector<HTMLElement>(".tb-body")!);
    await settle();
    expect(el.querySelector(".bulk-panel")).not.toBeNull();
  });
});

describe("bulk edit", () => {
  it("docks beside the list instead of opening above the whole window", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    click(rows(el)[1], { ctrlKey: true });
    await settle();
    key("E");
    await settle();
    const dock = el.querySelector(".bulk-dock")!;
    expect(dock).not.toBeNull();
    expect(dock.closest(".body")).not.toBeNull();
    expect(dock.querySelector(".bulk-panel")).not.toBeNull();
    // It takes the details panel's place, so the window does not get a third column.
    expect(el.querySelector("[data-testid='details-stub']")).toBeNull();
  });
});

describe("the book the details panel shows", () => {
  it("closes when a search hides it, so Save, Delete and Read cannot act on a book nobody can see", async () => {
    const el = await mountView();
    click(rows(el)[2]);
    await settle();
    expect(el.querySelector("[data-testid='details-stub']")!.textContent).toBe("details 3");
    await search(el, "title:one"); // only book 1
    expect(el.querySelector("[data-testid='details-stub']")).toBeNull();
  });

  it("stays when the search still shows it", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    await settle();
    await search(el, "title:one");
    expect(el.querySelector("[data-testid='details-stub']")!.textContent).toBe("details 1");
  });
});

describe("sorting", () => {
  it("remembers a header-click sort as the library's sort, keeping the other preferences", async () => {
    settingsMocks.fetchProfile.mockImplementation(async (name: string) => (name === "library-prefs" ? { sort: "timestamp", sortOrder: "desc", duplicateDefault: "skip" } : null));
    const el = await mountView();
    const title = el.querySelector<HTMLElement>("th[data-key='title'] .header-button")!;
    click(title);
    await settle();
    expect(settingsMocks.saveProfile).toHaveBeenCalledWith("library-prefs", expect.objectContaining({ sort: "title", sortOrder: "asc", duplicateDefault: "skip" }));
  });
});

describe("narrow windows", () => {
  it("keeps the table at full width and opens each side panel as a drawer over it", async () => {
    window.innerWidth = 900;
    const el = await mountView();
    expect(el.querySelector("[data-testid='categories-stub']")).toBeNull();
    click(rows(el)[0]);
    await settle();
    // A selected book does not squeeze the table with a details column.
    expect(el.querySelector("[data-testid='details-stub']")).toBeNull();
    const details = [...el.querySelectorAll<HTMLButtonElement>("footer .status-toggle")].find((b) => b.textContent === "Details")!;
    click(details);
    await settle();
    const drawer = el.querySelector(".details-column.details-drawer")!;
    expect(drawer).not.toBeNull();
    expect(drawer.querySelector("[data-testid='details-stub']")).not.toBeNull();
    expect(el.querySelector(".splitter")).toBeNull();
    // One drawer at a time.
    const categories = [...el.querySelectorAll<HTMLButtonElement>("footer .status-toggle")].find((b) => b.textContent === "Categories")!;
    click(categories);
    await settle();
    expect(el.querySelector("[data-testid='categories-stub']")).not.toBeNull();
    expect(el.querySelector(".details-column")).toBeNull();
  });

  it("is the three-column layout at a normal width", async () => {
    const el = await mountView();
    expect(el.querySelector("[data-testid='categories-stub']")).not.toBeNull();
    click(rows(el)[0]);
    await settle();
    expect(el.querySelector(".details-column")).not.toBeNull();
    expect(el.querySelector(".details-drawer")).toBeNull();
  });
});

describe("empty and offline states", () => {
  it("shows no table header over the empty-library card, and Add folder works in a browser", async () => {
    results = { "": [] };
    const el = await mountView();
    expect(el.querySelector("[data-testid='empty-library']")).not.toBeNull();
    expect(el.querySelector("table.book-table")).toBeNull();
    click(el.querySelector<HTMLElement>("[data-testid='empty-add-folder']")!);
    await settle();
    expect(document.querySelector("input[placeholder^='Full path of the folder']")).not.toBeNull();
  });

  it("says plainly that no books match, offers Clear search, and counts '0 of 6 books'", async () => {
    const el = await mountView();
    await search(el, "title:nothing");
    const empty = el.querySelector("[data-testid='empty-results']")!;
    expect(empty.getAttribute("role")).toBe("status");
    expect(empty.textContent).toContain("No books match your search");
    expect(empty.textContent).not.toContain("These are hiding");
    expect(el.querySelector("table.book-table")).toBeNull();
    expect(statusText(el)).toContain("0 of 6 books");
    const clear = empty.querySelector<HTMLButtonElement>("[data-testid='empty-clear-all']")!;
    expect(clear.textContent).toBe("Clear search");
    click(clear);
    await settle();
    expect(rows(el)).toHaveLength(6);
    expect(statusText(el)).toContain("6 books");
    expect(statusText(el)).not.toContain(" of ");
  });

  it("reports a dead server in words, as an alert, with a dash for the count and no editable panel", async () => {
    apiMocks.search.mockRejectedValue(new TypeError("Failed to fetch"));
    const el = await mountView();
    const empty = el.querySelector("[data-testid='empty-error']")!;
    expect(empty.getAttribute("role")).toBe("alert");
    expect(empty.textContent).toContain(UNREACHABLE);
    expect(empty.textContent).not.toContain("Failed to fetch");
    expect(statusText(el)).toContain("—");
    expect(statusText(el)).not.toContain("0 books");
    expect(el.querySelector("[data-testid='details-stub']")).toBeNull();
  });
});

describe("accessibility", () => {
  it("names the search box, and tells a screen reader the count changed", async () => {
    const el = await mountView();
    expect(el.querySelector("form.search input[type='search']")!.getAttribute("aria-label")).toBe("Search library");
    const count = el.querySelector(".status-count")!;
    expect(count.getAttribute("role")).toBe("status");
  });

  it("starts with a skip link that moves focus to the current row", async () => {
    const el = await mountView();
    const link = el.querySelector<HTMLAnchorElement>("a.skip-link")!;
    expect(link.textContent).toBe("Skip to book list");
    click(rows(el)[1]);
    await settle();
    click(link);
    await settle();
    expect(document.activeElement).toBe(rows(el)[1]);
  });
});
