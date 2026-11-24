// The details panel: collapsible sections, the always-on metadata form,
// the lazily loaded PDF preview (pdf.js mocked), and Show in folder.

import { createApp, defineComponent, h, nextTick, ref, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BookSummary } from "../library/types";
import { getFieldValues, invalidateFieldValues } from "../library/fieldValues";
import { sharedDrafts } from "../library/metadataForm";

const books: Record<number, BookSummary> = {};
const apiMocks = vi.hoisted(() => ({
  fetchBook: vi.fn(),
  setFields: vi.fn(),
  fetchFieldMetadata: vi.fn(),
  fetchDataFiles: vi.fn(),
}));
const pdfMocks = vi.hoisted(() => ({
  getDocument: vi.fn(),
  renderPage: vi.fn(),
  renderPageToBlob: vi.fn(),
}));

vi.mock("vue-router", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), ...apiMocks }));
vi.mock("../settings/api", () => ({ fetchProfile: vi.fn(async () => null), saveProfile: vi.fn(async () => undefined) }));
vi.mock("../reader/pdf", () => ({
  loadPdfJs: vi.fn(async () => ({ getDocument: pdfMocks.getDocument })),
  PDF_ASSET_OPTIONS: {},
  pdfUrl: (id: string) => `/get/pdf/${id}`,
  scaleForWidth: () => 1,
  renderPage: pdfMocks.renderPage,
  renderPageToBlob: pdfMocks.renderPageToBlob,
}));

import BookDetailsPanel from "./BookDetailsPanel.vue";
import { SECTIONS_KEY } from "../library/panelSections";

const makeBook = (id: number, formats: string[], title = `Book ${id}`): BookSummary =>
  ({
    id,
    title,
    authors: ["Ann Lee"],
    series: null,
    series_index: 1,
    tags: [],
    rating: 0,
    publisher: "",
    pubdate: null,
    comments: "",
    path: `Ann Lee/Book ${id} (${id})`,
    cover: `/get/cover/${id}`,
    thumbnail: `/get/thumb/${id}`,
    formats,
    main_format: formats.length ? { [formats[0]]: `/get/${formats[0]}/${id}` } : null,
    other_formats: {},
  }) as BookSummary;

let app: App | null = null;
let host: HTMLElement | null = null;

async function settle() {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await nextTick();
  }
  await new Promise((r) => setTimeout(r, 0));
  await nextTick();
}

/** Mounts the panel under a wrapper so the selected book can change. */
async function mountPanel(initial = 1, extra: Record<string, unknown> = {}) {
  host = document.createElement("div");
  document.body.appendChild(host);
  const bookId = ref(initial);
  const refreshKey = ref(0);
  const emitted: Record<string, unknown[][]> = {};
  const Wrapper = defineComponent({
    setup: () => () =>
      h(BookDetailsPanel, {
        bookId: bookId.value,
        refreshKey: refreshKey.value,
        docked: true,
        onUpdated: () => (emitted.updated ??= []).push([]),
        ...extra,
      }),
  });
  app = createApp(Wrapper);
  app.mount(host);
  await settle();
  return { el: host, bookId, refreshKey, emitted };
}

beforeEach(() => {
  localStorage.clear();
  sharedDrafts.clear();
  books[1] = makeBook(1, ["pdf"]);
  books[2] = makeBook(2, ["epub"]);
  apiMocks.fetchBook.mockImplementation(async (id: number) => structuredClone(books[id]));
  apiMocks.setFields.mockImplementation(async (id: number, changes: Record<string, unknown>) => ({ ...books[id], ...changes, authors: books[id].authors }));
  apiMocks.fetchFieldMetadata.mockResolvedValue({ field_metadata: {}, sortable_fields: [] });
  apiMocks.fetchDataFiles.mockResolvedValue({});
  pdfMocks.getDocument.mockImplementation(() => ({
    promise: Promise.resolve({ numPages: 3, getPage: async () => ({ getViewport: () => ({ width: 600 }) }), destroy: vi.fn() }),
  }));
  pdfMocks.renderPage.mockResolvedValue(undefined);
  pdfMocks.renderPageToBlob.mockResolvedValue(new Blob(["x"]));
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, headers: new Headers() })));
  URL.createObjectURL = vi.fn(() => "blob:page");
  URL.revokeObjectURL = vi.fn();
});

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const section = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-section='${id}']`);
const toggleOf = (el: HTMLElement, id: string) => section(el, id)!.querySelector<HTMLButtonElement>(".section-toggle")!;

describe("sections", () => {
  it("stacks cover, preview, metadata, files and actions, with several open at once", async () => {
    const { el } = await mountPanel();
    const ids = [...el.querySelectorAll("[data-section]")].map((s) => s.getAttribute("data-section"));
    expect(ids).toEqual(["cover", "preview", "metadata", "files", "actions"]);
    expect(section(el, "cover")!.classList.contains("open")).toBe(true);
    expect(section(el, "metadata")!.classList.contains("open")).toBe(true);
    expect(section(el, "files")!.classList.contains("open")).toBe(true);
    // No "Edit metadata" mode switch any more.
    expect(el.textContent).not.toContain("Edit metadata");
  });

  it("collapses a section on click and remembers it", async () => {
    const { el } = await mountPanel();
    toggleOf(el, "files").click();
    await nextTick();
    expect(section(el, "files")!.querySelector(".section-body")).toBeNull();
    expect(JSON.parse(localStorage.getItem(SECTIONS_KEY)!).files).toBe(false);
    expect(section(el, "metadata")!.querySelector(".section-body")).not.toBeNull();
  });

  it("restores collapsed state on the next mount", async () => {
    localStorage.setItem(SECTIONS_KEY, JSON.stringify({ cover: false }));
    const { el } = await mountPanel();
    expect(section(el, "cover")!.querySelector(".section-body")).toBeNull();
  });
});

describe("metadata form", () => {
  const titleInput = (el: HTMLElement) => el.querySelector<HTMLInputElement>("[data-field='title']")!;
  const save = (el: HTMLElement) => el.querySelector<HTMLButtonElement>("[data-action='save']")!;
  const revert = (el: HTMLElement) => el.querySelector<HTMLButtonElement>("[data-action='revert']")!;

  async function type(input: HTMLInputElement, value: string) {
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
  }

  it("is always there, with Save and Revert disabled until something changes", async () => {
    const { el } = await mountPanel();
    expect(titleInput(el).value).toBe("Book 1");
    expect(save(el).disabled).toBe(true);
    expect(revert(el).disabled).toBe(true);
    await type(titleInput(el), "Changed");
    expect(save(el).disabled).toBe(false);
    expect(revert(el).disabled).toBe(false);
  });

  it("reverts to the saved values", async () => {
    const { el } = await mountPanel();
    await type(titleInput(el), "Changed");
    revert(el).click();
    await nextTick();
    expect(titleInput(el).value).toBe("Book 1");
    expect(save(el).disabled).toBe(true);
  });

  it("saves through setFields and tells the list", async () => {
    const { el, emitted } = await mountPanel();
    await type(titleInput(el), "Changed");
    save(el).click();
    await settle();
    expect(apiMocks.setFields).toHaveBeenCalledTimes(1);
    expect(apiMocks.setFields.mock.calls[0][0]).toBe(1);
    // Only the edited field travels, so the save cannot revert anything else.
    expect(apiMocks.setFields.mock.calls[0][1]).toEqual({ title: "Changed" });
    expect(emitted.updated).toHaveLength(1);
    expect(save(el).disabled).toBe(true);
  });

  it("suggests known authors and tags while typing, and forgets them after a save", async () => {
    invalidateFieldValues();
    await getFieldValues("authors", async () => [{ name: "Jane Doe", count: 4 }, { name: "Ann Lovelace", count: 1 }]);
    const { el } = await mountPanel();
    const authors = el.querySelector<HTMLInputElement>("[data-field='authors']")!;
    expect(authors.getAttribute("role")).toBe("combobox");
    authors.focus();
    await type(authors, "Ann Lee & ja");
    await settle();
    const rows = [...document.body.querySelectorAll<HTMLElement>("[role='option']")];
    expect(rows.map((r) => r.textContent)).toEqual(["Jane Doe4"]);
    authors.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await nextTick();
    authors.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(authors.value).toBe("Ann Lee & Jane Doe & ");

    save(el).click();
    await settle();
    const loader = vi.fn(async () => []);
    await getFieldValues("authors", loader);
    expect(loader).toHaveBeenCalledTimes(1); // the save dropped the cached list
  });

  it("keeps a per-book draft when another book is selected, and restores it", async () => {
    const { el, bookId } = await mountPanel();
    await type(titleInput(el), "Draft title");
    bookId.value = 2;
    await settle();
    expect(titleInput(el).value).toBe("Book 2");
    expect(save(el).disabled).toBe(true);
    bookId.value = 1;
    await settle();
    expect(titleInput(el).value).toBe("Draft title");
    expect(save(el).disabled).toBe(false);
  });

  it("follows an edit made elsewhere (refreshKey) while the form is clean", async () => {
    const { el, refreshKey } = await mountPanel();
    books[1] = makeBook(1, ["pdf"], "Edited in the table");
    refreshKey.value++;
    await settle();
    expect(titleInput(el).value).toBe("Edited in the table");
    expect(save(el).disabled).toBe(true);
  });

  it("does not paint a saved book's data over the book selected while the save was in flight", async () => {
    const { el, bookId } = await mountPanel();
    let finish!: (b: BookSummary) => void;
    apiMocks.setFields.mockImplementationOnce(() => new Promise<BookSummary>((resolve) => (finish = resolve)));
    await type(titleInput(el), "A saved");
    save(el).click();
    await settle();
    bookId.value = 2; // arrow-key to the next row while the save runs
    await settle();
    expect(titleInput(el).value).toBe("Book 2");
    finish({ ...books[1], title: "A saved" });
    await settle();
    // Still book 2's data, and Save has nothing to write onto it.
    expect(titleInput(el).value).toBe("Book 2");
    expect(save(el).disabled).toBe(true);
    // The draft of book 1 was discarded by id, not carried to book 2.
    expect(sharedDrafts.has(1)).toBe(false);
    expect(sharedDrafts.has(2)).toBe(false);
  });

  it("ignores a refresh that arrives after the selection moved on", async () => {
    const { el, bookId, refreshKey } = await mountPanel();
    let finish!: (b: BookSummary) => void;
    apiMocks.fetchBook.mockImplementationOnce(() => new Promise<BookSummary>((resolve) => (finish = resolve)));
    refreshKey.value++;
    await settle();
    bookId.value = 2;
    await settle();
    finish(makeBook(1, ["pdf"], "Stale refresh of 1"));
    await settle();
    expect(titleInput(el).value).toBe("Book 2");
  });

  it("sends only the fields edited here, so a table edit made meanwhile is not reverted", async () => {
    const { el, refreshKey } = await mountPanel();
    // The title is edited in the table; the panel's baseline still has the old one.
    books[1] = makeBook(1, ["pdf"], "Edited in the table");
    refreshKey.value++;
    await settle();
    const publisher = el.querySelector<HTMLInputElement>("[data-field='publisher']")!;
    await type(publisher, "Acme");
    save(el).click();
    await settle();
    expect(apiMocks.setFields.mock.calls[0][1]).toEqual({ publisher: "Acme" });
  });

  it("keeps drafts when the panel unmounts, and restores them on the next mount", async () => {
    const first = await mountPanel();
    await type(titleInput(first.el), "Unsaved title");
    // Unmounting with a dirty form (the panel collapsed, the reader opened) stashes the draft.
    app?.unmount();
    host?.remove();
    expect(sharedDrafts.has(1)).toBe(true);
    const second = await mountPanel(1);
    expect(titleInput(second.el).value).toBe("Unsaved title");
  });

  it("keeps what was typed when the list edits the book meanwhile", async () => {
    const { el, refreshKey } = await mountPanel();
    await type(titleInput(el), "Typing…");
    books[1] = makeBook(1, ["pdf"], "Edited in the table");
    refreshKey.value++;
    await settle();
    expect(titleInput(el).value).toBe("Typing…");
  });

  it("offers Fetch metadata online inside the section", async () => {
    const { el } = await mountPanel();
    expect(section(el, "metadata")!.textContent).toContain("Fetch metadata online…");
  });
});

describe("preview", () => {
  it("does not touch pdf.js while the section is collapsed", async () => {
    await mountPanel();
    expect(pdfMocks.getDocument).not.toHaveBeenCalled();
    expect(pdfMocks.renderPage).not.toHaveBeenCalled();
  });

  it("renders page 1 once opened, and pages with the arrows", async () => {
    const { el } = await mountPanel();
    toggleOf(el, "preview").click();
    await settle();
    expect(pdfMocks.getDocument).toHaveBeenCalledTimes(1);
    expect(pdfMocks.renderPage).toHaveBeenCalledTimes(1);
    expect(pdfMocks.renderPage.mock.calls[0][1]).toBe(1);
    el.querySelector<HTMLButtonElement>("[aria-label='Next page']")!.click();
    await settle();
    expect(pdfMocks.renderPage.mock.calls[pdfMocks.renderPage.mock.calls.length - 1][1]).toBe(2);
    // The document is reused, not reloaded, for another page of the same book.
    expect(pdfMocks.getDocument).toHaveBeenCalledTimes(1);
  });

  it("loads the new book's PDF when the book changes while open, not before", async () => {
    books[3] = makeBook(3, ["pdf"]);
    const { el, bookId } = await mountPanel();
    toggleOf(el, "preview").click();
    await settle();
    bookId.value = 3;
    await settle();
    expect(pdfMocks.getDocument).toHaveBeenCalledTimes(2);
    expect(pdfMocks.renderPage.mock.calls[pdfMocks.renderPage.mock.calls.length - 1][1]).toBe(1);
  });

  it("shows a message instead of rendering for a book with no PDF", async () => {
    localStorage.setItem(SECTIONS_KEY, JSON.stringify({ preview: true }));
    const { el } = await mountPanel(2);
    expect(pdfMocks.getDocument).not.toHaveBeenCalled();
    expect(section(el, "preview")!.textContent).toContain("No preview for EPUB yet");
  });

  it("opens the zoomable viewer on a click, at high resolution", async () => {
    const { el } = await mountPanel();
    toggleOf(el, "preview").click();
    await settle();
    el.querySelector<HTMLCanvasElement>(".preview-canvas")!.click();
    await settle();
    expect(pdfMocks.renderPageToBlob).toHaveBeenCalledTimes(1);
    expect(pdfMocks.renderPageToBlob.mock.calls[0][2]).toBeGreaterThanOrEqual(2000);
    expect(document.querySelector(".viewer")).not.toBeNull();
    document.querySelector<HTMLButtonElement>(".viewer .close-viewer")!.click();
    await nextTick();
    expect(document.querySelector(".viewer")).toBeNull();
  });

  it("opens the cover in the viewer from the cover section", async () => {
    const { el } = await mountPanel();
    el.querySelector<HTMLButtonElement>(".cover-button")!.click();
    await nextTick();
    expect(document.querySelector(".viewer")).not.toBeNull();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await nextTick();
    expect(document.querySelector(".viewer")).toBeNull();
  });
});

describe("files and Show in folder", () => {
  it("lists each format with the book's folder path", async () => {
    const { el } = await mountPanel();
    expect(section(el, "files")!.querySelector("[data-format='pdf']")).not.toBeNull();
    expect(el.querySelector("[data-testid='folder-path']")!.textContent).toBe("Ann Lee/Book 1 (1)");
  });

  it("hides Show in folder in a plain browser", async () => {
    const { el } = await mountPanel();
    expect(el.querySelector("[data-action='show-in-folder']")).toBeNull();
  });

  it("asks the desktop app to reveal that file when run inside it", async () => {
    const invoke = vi.fn(async () => undefined);
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = { invoke };
    const { el } = await mountPanel();
    el.querySelector<HTMLButtonElement>("[data-action='show-in-folder']")!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith("reveal_book_file", { bookId: 1, fmt: "pdf" });
  });
});
