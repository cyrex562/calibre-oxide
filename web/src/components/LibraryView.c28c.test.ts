// The library view's wiring for the round-2 usability fixes (tranche 28c): what a merge leaves
// open, Merge in the right-click menu, Save this search, and the narrow layout's drawers.

import { createApp, defineComponent, h, KeepAlive, nextTick, ref, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BookSummary } from "../library/types";

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
  setSavedSearch: vi.fn(),
  setVirtualLibrary: vi.fn(),
  mergeBooks: vi.fn(),
}));

vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), ...apiMocks }));
vi.mock("../settings/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../settings/api")>()), fetchProfile: vi.fn(async () => null), saveProfile: vi.fn(async () => undefined) }));
vi.mock("../tauri", () => ({ isTauri: () => false, tauriInvoke: vi.fn(async () => undefined) }));
vi.mock("./BookDetailsPanel.vue", () => ({ default: defineComponent({ props: ["bookId"], setup: (p) => () => h("aside", { "data-testid": "details-stub" }, `details ${p.bookId}`) }) }));
vi.mock("./CategoryBrowser.vue", () => ({ default: defineComponent({ props: ["virtualLibrary"], setup: (p) => () => h("nav", { "data-testid": "categories-stub" }, p.virtualLibrary ? `vl ${p.virtualLibrary}` : "") }) }));

import LibraryView from "./LibraryView.vue";
import { dismissToast } from "../library/deleteFlow";
import { markLibraryChanged } from "../library/libraryEpoch";

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
/** Ids the library no longer has (a merge deleted them). */
let gone = new Set<number>();
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
  sessionStorage.clear();
  gone = new Set();
  results = { "": [1, 2, 3, 4, 5, 6], "title:one": [1] };
  apiMocks.search.mockImplementation(async (p: { query: string }) => {
    const ids = (results[p.query] ?? []).filter((id) => !gone.has(id));
    return { book_ids: ids, total_num: ids.length };
  });
  apiMocks.fetchBooks.mockImplementation(async (ids: number[]) => ids.filter((id) => !gone.has(id)).map(makeBook));
  apiMocks.fetchFieldMetadata.mockResolvedValue({ field_metadata: {}, sortable_fields: [["title", "Title"]] });
  apiMocks.fetchVirtualLibraries.mockResolvedValue({});
  apiMocks.fetchSavedSearches.mockResolvedValue({});
  apiMocks.fetchLibraryInfo.mockResolvedValue({ name: "D:\\Books\\scratch", path: "D:/Books/scratch", id: "scratch", storage: null, libraries: {} });
  apiMocks.fetchLibraryJobs.mockResolvedValue([]);
  apiMocks.fetchUntracked.mockResolvedValue({ untracked: 0, capped: false });
  apiMocks.fetchTrash.mockResolvedValue({ count: 0, total_size: 0, expire_after_seconds: 0, expired: [], entries: [] });
  apiMocks.setSavedSearch.mockResolvedValue(undefined);
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404, statusText: "Not found", headers: new Headers(), text: async () => "" })));
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  vi.spyOn(console, "warn").mockImplementation(() => undefined);
});

afterEach(() => {
  dismissToast();
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("tbody tr[data-id]:not(.placeholder)")];
const click = (row: HTMLElement, init: MouseEventInit = {}) => row.dispatchEvent(new MouseEvent("click", { bubbles: true, ...init }));
const menuLabels = () => Array.from(document.body.querySelectorAll('[role="menu"] [role="menuitem"]')).map((e) => e.textContent!.trim());
const details = (el: HTMLElement) => el.querySelector('[data-testid="details-stub"]')?.textContent ?? null;

describe("Merge books (C8, C3)", () => {
  async function selectTwo(el: HTMLElement) {
    click(rows(el)[0]);
    await settle();
    click(rows(el)[1], { ctrlKey: true });
    await settle();
  }

  it("is in the right-click menu when two books are selected, and not for one", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    await settle();
    rows(el)[0].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    await settle();
    const merge = () => Array.from(document.body.querySelectorAll<HTMLButtonElement>('[role="menu"] [role="menuitem"]')).find((e) => /Merge books/.test(e.textContent!));
    expect(merge()?.disabled ?? true).toBe(true); // greyed out (or absent) with one book
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();

    await selectTwo(el);
    rows(el)[1].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    await settle();
    expect(menuLabels()).toContain("Merge books…");
    expect(menuLabels()).toContain("Compare as duplicates…");
    expect(merge()!.disabled).toBe(false);
  });

  it("drops the merged-away book from the details panel and shows the one that was kept", async () => {
    apiMocks.mergeBooks.mockImplementation(async (req: { dry_run?: boolean; sources: number[]; dest: number }) => {
      if (!req.dry_run) gone.add(req.sources[0]);
      return {
        ok: true,
        dry_run: !!req.dry_run,
        dest: req.dest,
        mode: "merge_and_delete",
        results: req.sources.map((id) => ({ book_id: id, title: `Book ${id}`, formats_moved: [], formats_identical: [], formats_discarded: [], formats_kept_as_data_file: [], formats_left_on_source: [], data_files_moved: [], deleted: true, trashed: [], failed: [], error: null, warnings: [] })),
        metadata_changed: [],
        cover_from: null,
        trashed: [],
        failed: [],
        errors: [],
        warnings: [],
      };
    });
    const el = await mountView();
    await selectTwo(el);
    // The last book clicked is the one open in the panel.
    expect(details(el)).toBe("details 2");

    rows(el)[1].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    await settle();
    Array.from(document.body.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]')).find((e) => /Merge books/.test(e.textContent!))!.click();
    await settle();
    const primary = () => document.body.querySelector<HTMLButtonElement>(".dialog-footer button.primary")!;
    primary().click(); // Review
    await settle();
    primary().click(); // Merge
    await settle();

    expect(apiMocks.mergeBooks).toHaveBeenLastCalledWith(expect.objectContaining({ dest: 1, sources: [2] }));
    // Book 2 no longer exists: the panel shows the kept record, and nothing is left selected.
    expect(details(el)).toBe("details 1");
    expect(el.querySelector("footer .status-sel")?.textContent ?? "").not.toMatch(/selected/);
  });
});

describe("a book that vanishes from the library", () => {
  /** The library view inside <keep-alive>, parked as the reader parks it. */
  async function mountKept() {
    host = document.createElement("div");
    document.body.appendChild(host);
    const away = ref(false);
    app = createApp({ render: () => h(KeepAlive, null, { default: () => (away.value ? h("div", { "data-testid": "reader" }) : h(LibraryView)) }) });
    app.component("router-link", RouterLinkStub);
    app.mount(host);
    await settle();
    return { el: host, away };
  }

  it("closes the details panel on the next refresh when the library no longer has the book", async () => {
    const { el, away } = await mountKept();
    click(rows(el)[2]);
    await settle();
    expect(details(el)).toBe("details 3");

    away.value = true;
    await settle();
    gone.add(3); // deleted elsewhere, say by a resolve in another window
    markLibraryChanged();
    away.value = false;
    await settle();
    expect(details(el)).toBeNull();
  });

  it("keeps the panel open for a book a refresh only filtered out of the list", async () => {
    const { el, away } = await mountKept();
    click(rows(el)[2]);
    await settle();
    away.value = true;
    await settle();
    // Still in the library (fetchBooks has it), just not in the list any more.
    results[""] = [1, 2, 4, 5, 6];
    markLibraryChanged();
    away.value = false;
    await settle();
    expect(details(el)).toBe("details 3");
  });
});

describe("Save this search (C19)", () => {
  it("offers a button once a search is on, names it, saves it and reloads the menus", async () => {
    const el = await mountView();
    expect(el.querySelector('[data-testid="save-search"]')).toBeNull();
    const input = el.querySelector<HTMLInputElement>("form.search input[type='search']")!;
    input.value = "title:one";
    input.dispatchEvent(new Event("input"));
    el.querySelector<HTMLFormElement>("form.search")!.dispatchEvent(new Event("submit", { cancelable: true }));
    await settle();

    const before = apiMocks.fetchSavedSearches.mock.calls.length;
    apiMocks.fetchSavedSearches.mockResolvedValue({ Mine: "title:one" });
    el.querySelector<HTMLButtonElement>('[data-testid="save-search"]')!.click();
    await settle();
    const name = document.body.querySelector<HTMLInputElement>("[data-testid=confirm-input]")!;
    name.value = "Mine";
    name.dispatchEvent(new Event("input"));
    document.body.querySelector<HTMLButtonElement>("[data-testid=confirm-ok]")!.click();
    await settle();

    expect(apiMocks.setSavedSearch).toHaveBeenCalledWith("Mine", "title:one");
    expect(apiMocks.fetchSavedSearches.mock.calls.length).toBeGreaterThan(before);
  });
});

describe("the category counts under a virtual library (C19)", () => {
  it("tell the panel which virtual library is on, so it can say the counts are library-wide", async () => {
    apiMocks.fetchVirtualLibraries.mockResolvedValue({ Travel: "tags:travel" });
    const el = await mountView();
    expect(el.querySelector('[data-testid="categories-stub"]')!.textContent).toBe("");
    el.querySelector<HTMLButtonElement>('button[title^="Virtual library"]')!.click();
    await settle();
    Array.from(document.body.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitemradio"], [role="menu"] [role="menuitem"]')).find((e) => e.textContent!.trim() === "Travel")!.click();
    await settle();
    expect(el.querySelector('[data-testid="categories-stub"]')!.textContent).toBe("vl Travel");
  });
});

describe("the narrow layout's drawers (C25)", () => {
  it("takes focus when opened, closes on Escape or a click outside, and gives focus back", async () => {
    window.innerWidth = 900;
    const el = await mountView();
    click(rows(el)[0]);
    await settle();
    const toggle = Array.from(el.querySelectorAll<HTMLButtonElement>("footer .status-toggle")).find((b) => b.textContent === "Details")!;
    toggle.focus();
    toggle.click();
    await settle();
    const drawer = el.querySelector<HTMLElement>('[data-drawer="details"]')!;
    expect(drawer).not.toBeNull();
    expect(document.activeElement).toBe(drawer);
    expect(el.querySelector('[data-testid="drawer-backdrop"]')).not.toBeNull();

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(el.querySelector('[data-drawer="details"]')).toBeNull();
    expect(document.activeElement).toBe(toggle);

    toggle.click();
    await settle();
    el.querySelector<HTMLElement>('[data-testid="drawer-backdrop"]')!.click();
    await settle();
    expect(el.querySelector('[data-drawer="details"]')).toBeNull();
  });
});

describe("the library name (C28)", () => {
  it("is the last folder on its button, with the whole path in the tooltip", async () => {
    const el = await mountView();
    const button = el.querySelector<HTMLButtonElement>(".status-library")!;
    expect(button.textContent).toContain("scratch");
    expect(button.textContent).not.toContain("Books");
    expect(button.getAttribute("aria-label")).toBe("Library: scratch. Switch library");
    expect(button.title).toContain("D:/Books/scratch");
  });
});
