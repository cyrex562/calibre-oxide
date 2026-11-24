// The library view, mounted with the network mocked: selection, the delete
// flow, shortcuts behind dialogs, empty states, listener hygiene and the
// native-menu sync. The pure parts of each are tested in src/library/*.test.ts;
// these pin the wiring, which is where the regressions were.

import { createApp, defineComponent, h, KeepAlive, nextTick, ref, type App } from "vue";
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
  deleteBooks: vi.fn(),
  restoreDeletedBooks: vi.fn(),
  setFields: vi.fn(),
}));
const tauriMocks = vi.hoisted(() => ({ invoke: vi.fn(async (_cmd: string, _args?: unknown): Promise<undefined> => undefined) }));

vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), ...apiMocks }));
vi.mock("../settings/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../settings/api")>()), fetchProfile: vi.fn(async () => null), saveProfile: vi.fn(async () => undefined) }));
vi.mock("../tauri", () => ({ isTauri: () => state.tauri, tauriInvoke: tauriMocks.invoke }));
vi.mock("./BookDetailsPanel.vue", () => ({ default: defineComponent({ props: ["bookId"], setup: (p) => () => h("aside", { "data-testid": "details-stub" }, `details ${p.bookId}`) }) }));
vi.mock("./CategoryBrowser.vue", () => ({ default: defineComponent({ setup: () => () => h("nav", { "data-testid": "categories-stub" }) }) }));
vi.mock("./TrashDialog.vue", () => ({
  default: defineComponent({ emits: ["close"], setup: () => () => h("div", { class: "manage-backdrop" }, [h("div", { role: "dialog", "aria-modal": "true", "data-testid": "trash-stub" })]) }),
}));

import LibraryView from "./LibraryView.vue";
import { answerTrashConfirm, confirmState, dismissToast, toastState } from "../library/deleteFlow";
import { MENU_ACTION_EVENT } from "../library/desktopMenu";
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

/** The ids each query returns; anything else matches nothing. */
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

function unmountView() {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
}

beforeEach(() => {
  // jsdom's window is 1024px wide, which is "narrow" (side panels as drawers); these tests are the wide layout.
  window.innerWidth = 1400;
  localStorage.clear();
  sessionStorage.clear();
  state.tauri = false;
  results = { "": [1, 2, 3, 4, 5, 6], "title:one": [1] };
  apiMocks.search.mockImplementation(async (p: { query: string }) => ({ book_ids: results[p.query] ?? [], total_num: (results[p.query] ?? []).length }));
  apiMocks.fetchBooks.mockImplementation(async (ids: number[]) => ids.map(makeBook));
  apiMocks.fetchFieldMetadata.mockResolvedValue({ field_metadata: {}, sortable_fields: [] });
  apiMocks.fetchVirtualLibraries.mockResolvedValue({});
  apiMocks.fetchSavedSearches.mockResolvedValue({});
  apiMocks.fetchLibraryInfo.mockResolvedValue({ library_name: "scratch", library_path: "C:/scratch" });
  apiMocks.fetchLibraryJobs.mockResolvedValue([]);
  apiMocks.fetchUntracked.mockResolvedValue({ untracked: 0, capped: false });
  apiMocks.fetchTrash.mockResolvedValue({ count: 0, total_size: 0, expire_after_seconds: 0, expired: [], entries: [] });
  apiMocks.deleteBooks.mockImplementation(async (ids: number[]) => ({ deleted: ids, failed: [] }));
  apiMocks.restoreDeletedBooks.mockResolvedValue(1);
  tauriMocks.invoke.mockClear();
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404, statusText: "Not found", headers: new Headers(), text: async () => "" })));
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  vi.spyOn(console, "warn").mockImplementation(() => undefined);
});

afterEach(() => {
  answerTrashConfirm(false);
  dismissToast();
  unmountView();
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("tbody tr[data-id]:not(.placeholder)")];
const click = (row: HTMLElement, init: MouseEventInit = {}) => row.dispatchEvent(new MouseEvent("click", { bubbles: true, ...init }));
const key = (k: string, init: KeyboardEventInit = {}, target: EventTarget = window) => target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
const statusText = (el: HTMLElement) => el.querySelector("footer")?.textContent ?? "";

async function search(el: HTMLElement, text: string) {
  const input = el.querySelector<HTMLInputElement>("form.search input[type='search']")!;
  input.value = text;
  input.dispatchEvent(new Event("input"));
  el.querySelector<HTMLFormElement>("form.search")!.dispatchEvent(new Event("submit", { cancelable: true }));
  await settle();
}

describe("selecting rows", () => {
  it("Ctrl-click adds and removes rows, Shift-click selects a range, Ctrl+A selects everything, outside select mode", async () => {
    const el = await mountView();
    expect(rows(el)).toHaveLength(6);
    click(rows(el)[1]);
    await settle();
    click(rows(el)[3], { ctrlKey: true });
    await settle();
    expect(statusText(el)).toContain("2 selected");
    click(rows(el)[3], { ctrlKey: true });
    await settle();
    expect(statusText(el)).toContain("1 selected");
    click(rows(el)[1]);
    click(rows(el)[4], { shiftKey: true });
    await settle();
    expect(statusText(el)).toContain("4 selected");
    rows(el)[2].focus();
    key("a", { ctrlKey: true }, rows(el)[2]);
    await settle();
    expect(statusText(el)).toContain("6 selected");
  });

  it("Shift+arrow extends the selection from where the walk started", async () => {
    const el = await mountView();
    click(rows(el)[1]);
    await settle();
    key("ArrowDown", { shiftKey: true }, rows(el)[1]);
    key("ArrowDown", { shiftKey: true }, rows(el)[1]);
    await settle();
    expect(statusText(el)).toContain("3 selected");
    // A plain arrow collapses it back to one row.
    key("ArrowDown", {}, rows(el)[1]);
    await settle();
    expect(statusText(el)).not.toContain("selected");
  });

  it("drops selected books a new search no longer shows, so the delete confirm cannot count hidden ones", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    click(rows(el)[1], { ctrlKey: true });
    click(rows(el)[2], { ctrlKey: true });
    await settle();
    expect(statusText(el)).toContain("3 selected");
    await search(el, "title:one"); // matches book 1 only
    expect(statusText(el)).toContain("1 selected");
    key("Delete");
    await settle();
    expect(confirmState.open).toBe(true);
    expect(confirmState.text?.question).toBe("Move 1 book to the library Trash?");
    expect(confirmState.text?.hiddenNote).toBeNull();
  });
});

describe("delete", () => {
  it("names the titles, moves the books to the Trash, and offers Undo", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    click(rows(el)[1], { ctrlKey: true });
    await settle();
    key("Delete");
    await settle();
    expect(confirmState.text?.titles).toEqual(["Book 1", "Book 2"]);
    expect(document.body.textContent).toContain("Move 2 books to the library Trash?");
    answerTrashConfirm(true);
    await settle();
    expect(apiMocks.deleteBooks).toHaveBeenCalledWith([1, 2]);
    expect(toastState.toast?.message).toBe("Moved 2 books to Trash");
    expect(document.body.textContent).toContain("Moved 2 books to Trash");
    expect(document.body.textContent).toContain("Undo");
  });

  it("shows what the server could not delete", async () => {
    apiMocks.deleteBooks.mockResolvedValue({ deleted: [1], failed: [{ book_id: 2, error: "file is in use" }] });
    const el = await mountView();
    click(rows(el)[0]);
    click(rows(el)[1], { ctrlKey: true });
    await settle();
    key("Delete");
    await settle();
    answerTrashConfirm(true);
    await settle();
    expect(el.textContent).toContain("1 could not be deleted");
    expect(el.textContent).toContain("file is in use");
  });

  it("does not delete the library book behind an open dialog when Del is pressed", async () => {
    const el = await mountView();
    click(rows(el)[0]);
    await settle();
    window.dispatchEvent(new CustomEvent(MENU_ACTION_EVENT, { detail: { id: "trash" } }));
    await settle();
    expect(document.querySelector("[data-testid='trash-stub']")).not.toBeNull();
    key("Delete");
    await settle();
    expect(confirmState.open).toBe(false);
    expect(apiMocks.deleteBooks).not.toHaveBeenCalled();
  });
});

describe("empty states", () => {
  it("offers to add books when the library is empty", async () => {
    results = { "": [] };
    const el = await mountView();
    expect(el.querySelector("[data-testid='empty-library']")).not.toBeNull();
    expect(el.textContent).toContain("Add books…");
    expect(el.textContent).toContain("Add folder…");
    expect(el.querySelector("[data-testid='empty-results']")).toBeNull();
  });

  it("shows the active filter with a one-click clear when a search matches nothing", async () => {
    const el = await mountView();
    await search(el, "title:nothing");
    const empty = el.querySelector("[data-testid='empty-results']");
    expect(empty).not.toBeNull();
    expect(el.querySelector("[data-testid='empty-library']")).toBeNull();
    expect(empty!.textContent).toContain("Search: title:nothing");
    empty!.querySelector<HTMLButtonElement>(".filter-chip button")!.click();
    await settle();
    expect(rows(el)).toHaveLength(6);
  });

  it("shows a Retry when the list fails to load, and retries", async () => {
    apiMocks.search.mockRejectedValueOnce(new Error("server unreachable"));
    const el = await mountView();
    expect(el.querySelector("[data-testid='empty-error']")!.textContent).toContain("server unreachable");
    el.querySelector<HTMLButtonElement>("[data-testid='empty-retry']")!.click();
    await settle();
    expect(rows(el)).toHaveLength(6);
    expect(el.querySelector("[data-testid='empty-error']")).toBeNull();
  });
});

describe("window listeners", () => {
  it("opens one file picker per menu 'Add books', however many times the view was mounted", async () => {
    const clicks = vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(() => undefined);
    await mountView();
    unmountView();
    await mountView();
    unmountView();
    await mountView();
    clicks.mockClear();
    window.dispatchEvent(new CustomEvent(MENU_ACTION_EVENT, { detail: { id: "add-books" } }));
    await settle();
    expect(clicks).toHaveBeenCalledTimes(1);
  });

  it("removes its listeners when it unmounts", async () => {
    const added = vi.spyOn(window, "addEventListener");
    const removed = vi.spyOn(window, "removeEventListener");
    await mountView();
    unmountView();
    for (const name of ["lexmechanic:books-added", MENU_ACTION_EVENT, "keydown", "beforeunload"]) {
      const adds = added.mock.calls.filter(([n]) => n === name).length;
      const removes = removed.mock.calls.filter(([n]) => n === name).length;
      expect(adds, `${name} was never added`).toBeGreaterThan(0);
      expect(removes, `${name} is added ${adds}x but removed ${removes}x`).toBe(adds);
    }
  });
});

describe("the native menu", () => {
  const menuCalls = () => tauriMocks.invoke.mock.calls.filter(([cmd]) => cmd === "set_menu_actions").length;

  it("is rebuilt when what is enabled changes, not on every selection change", async () => {
    state.tauri = true;
    const el = await mountView();
    const initial = menuCalls();
    expect(initial).toBeGreaterThan(0);
    click(rows(el)[0]); // nothing -> one book: enablement changes
    await settle();
    const afterOne = menuCalls();
    expect(afterOne).toBeGreaterThan(initial);
    click(rows(el)[1], { ctrlKey: true }); // one -> two: multi-selection actions turn on
    await settle();
    const afterTwo = menuCalls();
    click(rows(el)[2], { ctrlKey: true }); // two -> three: nothing changes
    click(rows(el)[3], { ctrlKey: true });
    click(rows(el)[4], { ctrlKey: true });
    await settle();
    expect(menuCalls()).toBe(afterTwo);
  });
});

describe("drafts", () => {
  it("counts unsaved drafts in the status bar", async () => {
    const { sharedDrafts } = await import("../library/metadataForm");
    sharedDrafts.clear();
    const el = await mountView();
    expect(el.querySelector("[data-testid='draft-count']")).toBeNull();
    const base = { title: "a", authors: "", series: "", seriesIndex: "", tags: "", rating: 0, publisher: "", pubdate: "", comments: "", custom: {} };
    sharedDrafts.stash(1, { ...base, title: "changed" }, base);
    await settle();
    expect(el.querySelector("[data-testid='draft-count']")!.textContent).toContain("1 unsaved draft");
    sharedDrafts.clear();
  });
});

describe("kept alive behind another route", () => {
  /** The library view inside <keep-alive>, with a switch that parks it as the reader would. */
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

  it("keeps the query, the rows and the selection, and does not reload when nothing changed", async () => {
    const { el, away } = await mountKept();
    await search(el, "title:one");
    click(rows(el)[0]);
    await settle();
    expect(rows(el)[0].classList.contains("selected")).toBe(true);
    const searches = apiMocks.search.mock.calls.length;

    away.value = true;
    await settle();
    expect(el.querySelector('[data-testid="reader"]')).not.toBeNull();
    away.value = false;
    await settle();

    expect(el.querySelector<HTMLInputElement>("form.search input[type='search']")!.value).toBe("title:one");
    expect(rows(el)).toHaveLength(1);
    expect(rows(el)[0].classList.contains("selected")).toBe(true);
    expect(el.querySelector('[data-testid="details-stub"]')?.textContent).toBe("details 1");
    expect(apiMocks.search.mock.calls.length).toBe(searches);
  });

  it("does not react to Del or menu actions while parked, and does again once shown", async () => {
    const { el, away } = await mountKept();
    click(rows(el)[0]);
    await settle();
    away.value = true;
    await settle();

    key("Delete");
    window.dispatchEvent(new CustomEvent(MENU_ACTION_EVENT, { detail: { id: "delete-book" } }));
    await settle();
    expect(confirmState.open).toBe(false);

    away.value = false;
    await settle();
    key("Delete");
    await settle();
    expect(confirmState.open).toBe(true);
  });

  it("reloads its list on return when something was written while it was away", async () => {
    const { el, away } = await mountKept();
    const searches = apiMocks.search.mock.calls.length;
    away.value = true;
    await settle();
    markLibraryChanged();
    away.value = false;
    await settle();
    expect(apiMocks.search.mock.calls.length).toBeGreaterThan(searches);
    expect(rows(el)).toHaveLength(6);
  });
});
