// Every dialog taken out of the library view: it opens as a modal dialog, puts
// its buttons in the footer (primary at the right, Cancel next to it), reports
// what it did through events, and closes on Close/Cancel.

import { createApp, h, nextTick, reactive, ref, type App } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import BulkEditPanel from "../BulkEditPanel.vue";
import CheckLibraryDialog from "./CheckLibraryDialog.vue";
import ColumnPickerDialog from "./ColumnPickerDialog.vue";
import CopyToLibraryDialog from "./CopyToLibraryDialog.vue";
import CustomColumnsDialog from "./CustomColumnsDialog.vue";
import DuplicatesDialog from "./DuplicatesDialog.vue";
import EmbedMetadataDialog from "./EmbedMetadataDialog.vue";
import ManageListsDialog from "./ManageListsDialog.vue";
import MergeBooksDialog from "./MergeBooksDialog.vue";
import NewsDialog from "./NewsDialog.vue";
import RenameFilesDialog from "./RenameFilesDialog.vue";
import SaveToDiskDialog from "./SaveToDiskDialog.vue";
import type { BookColumn } from "../../library/columns";
import { createDuplicatesStore } from "../../library/duplicatesStore";
import { emptySpec, type BulkEdit } from "../../library/useBulkEdit";

const api = vi.hoisted(() => ({
  renameFiles: vi.fn(),
  embedMetadata: vi.fn(),
  copyToLibrary: vi.fn(),
  saveToDisk: vi.fn(),
  checkLibrary: vi.fn(),
  fetchOrphans: vi.fn(),
  fetchIgnored: vi.fn(),
  fetchCustomColumns: vi.fn(),
  listNewsSchedules: vi.fn(),
  mergeBooks: vi.fn(),
  setVirtualLibrary: vi.fn(),
  fetchNaming: vi.fn(),
}));
vi.mock("../../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../library/api")>()), ...api }));
vi.mock("../../settings/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../settings/api")>()), fetchProfile: async () => null, saveProfile: async () => {} }));

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  document.body.replaceChildren();
  for (const m of Object.values(api)) m.mockReset();
});

async function mount(component: object, props: Record<string, unknown>) {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(component, props);
  app.mount(host);
  await settle();
}

async function settle() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

const dialog = () => document.body.querySelector<HTMLElement>("[role=dialog]");
const footerButtons = () => Array.from(document.body.querySelectorAll<HTMLButtonElement>(".dialog-footer button"));
const labels = () => footerButtons().map((b) => b.textContent!.trim());

describe("the dialogs taken out of the library view", () => {
  it("Rename files: Preview, Close, then Rename at the right; it previews and applies", async () => {
    api.renameFiles.mockImplementation(async (_ids: number[], template: string, dry: boolean) => ({ results: [{ book_id: 1, title: "T", current: "old", proposed: dry && template === "{title}" ? "old" : "new", changed: template !== "{title}", error: null }] }));
    api.fetchNaming.mockResolvedValue({ file_template: "{title}" });
    const renamed = vi.fn();
    const closed = vi.fn();
    await mount(RenameFilesDialog, { ids: [1], onRenamed: renamed, onClose: closed });
    expect(dialog()).not.toBeNull();
    expect(labels()).toEqual(["Preview", "Close", "Rename"]);
    const rename = footerButtons()[2];
    expect(rename.disabled).toBe(true);
    const input = document.body.querySelector<HTMLInputElement>("input")!;
    input.value = "{title} - {authors}";
    input.dispatchEvent(new Event("input"));
    footerButtons()[0].click();
    await settle();
    expect(rename.disabled).toBe(false);
    rename.click();
    await settle();
    expect(renamed).toHaveBeenCalledTimes(1);
    expect(api.renameFiles).toHaveBeenLastCalledWith([1], "{title} - {authors}", false);
    footerButtons()[1].click();
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("Embed metadata: Cancel then Embed, then only Close once it ran", async () => {
    api.embedMetadata.mockResolvedValue({ books: { "1": { formats: { EPUB: { status: "embedded" } } } } });
    const embedded = vi.fn();
    await mount(EmbedMetadataDialog, { ids: [1, 2], onEmbedded: embedded });
    expect(labels()).toEqual(["Cancel", "Embed into 2 book(s)"]);
    footerButtons()[1].click();
    await settle();
    expect(api.embedMetadata).toHaveBeenCalledWith([1, 2]);
    expect(embedded).toHaveBeenCalledTimes(1);
    expect(labels()).toEqual(["Close"]);
  });

  it("Copy to library: says so when there is nowhere to copy, and copies when there is", async () => {
    await mount(CopyToLibraryDialog, { ids: [1], targets: [] });
    expect(dialog()!.textContent).toContain("nowhere to copy to");
    expect(labels()).toEqual(["Close"]);
    app!.unmount();
    host!.remove();
    api.copyToLibrary.mockResolvedValue({ "1": { status: "copied", new_book_id: 9 } });
    await mount(CopyToLibraryDialog, { ids: [1, 2], targets: [{ id: "b", name: "Other" }] });
    expect(labels()).toEqual(["Close", "Copy 2 book(s)"]);
    footerButtons()[1].click();
    await settle();
    expect(api.copyToLibrary).toHaveBeenCalledWith("b", [1, 2], false);
  });

  it("Save to disk: Close then Save, which needs a destination", async () => {
    api.saveToDisk.mockResolvedValue({ results: [{ book_id: 1, ok: true, paths: ["a"] }] });
    await mount(SaveToDiskDialog, { ids: [1] });
    expect(labels()).toEqual(["Close", "Save"]);
    expect(footerButtons()[1].disabled).toBe(true);
    const dest = document.body.querySelector<HTMLInputElement>("input[placeholder='/home/me/Books']")!;
    dest.value = "/tmp/out";
    dest.dispatchEvent(new Event("input"));
    await settle();
    footerButtons()[1].click();
    await settle();
    expect(api.saveToDisk).toHaveBeenCalledTimes(1);
    expect(api.saveToDisk.mock.calls[0][0]).toEqual([1]);
    expect(api.saveToDisk.mock.calls[0][2]).toBe("/tmp/out");
  });

  it("Merge books: Cancel then Review, which becomes Merge and reports the result", async () => {
    api.mergeBooks.mockImplementation(async (req: { dry_run?: boolean }) => ({ ok: true, dry_run: !!req.dry_run, dest: 1, mode: "merge_and_delete", results: [], metadata_changed: [], cover_from: null, trashed: [], failed: [], errors: [], warnings: [] }));
    const merged = vi.fn();
    const books = [
      { book_id: 1, title: "A", authors: ["X"], formats: 1 },
      { book_id: 2, title: "A", authors: ["X"], formats: 1 },
    ];
    await mount(MergeBooksDialog, { books, dest: 1, onMerged: merged });
    expect(labels()).toEqual(["Cancel", "Review…"]);
    footerButtons()[1].click();
    await settle();
    expect(api.mergeBooks).toHaveBeenLastCalledWith(expect.objectContaining({ dest: 1, sources: [2], dry_run: true }));
    expect(labels()[1]).toBe("Merge");
  });

  it("Table columns: reports toggles, moves and a drop, with Reset and a primary Close", async () => {
    const columns = [
      { key: "title", label: "Title", sortKey: "title" },
      { key: "authors", label: "Authors", sortKey: "authors" },
      { key: "size", label: "Size", sortKey: null },
    ] as unknown as BookColumn[];
    const toggle = vi.fn();
    const move = vi.fn();
    const reset = vi.fn();
    const reorder = vi.fn();
    const close = vi.fn();
    await mount(ColumnPickerDialog, { columns, shown: ["title", "authors"], onToggle: toggle, onMove: move, onReset: reset, onReorder: reorder, onClose: close });
    expect(labels()).toEqual(["Reset to default", "Close"]);
    const rows = document.body.querySelectorAll<HTMLElement>(".column-row");
    expect(rows).toHaveLength(3);
    rows[0].querySelector<HTMLInputElement>("input[type=checkbox]")!.dispatchEvent(new Event("change"));
    expect(toggle).toHaveBeenCalledWith("title");
    rows[0].querySelector<HTMLButtonElement>("button[aria-label='Move down']")!.click();
    expect(move).toHaveBeenCalledWith("title", 1);
    rows[0].dispatchEvent(new Event("dragstart"));
    rows[1].dispatchEvent(new Event("drop", { cancelable: true }));
    expect(reorder).toHaveBeenCalledWith("title", "authors");
    footerButtons()[0].click();
    footerButtons()[1].click();
    expect(reset).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("Virtual libraries and saved searches: lists both and reports an added list", async () => {
    api.setVirtualLibrary.mockResolvedValue(undefined);
    const changed = vi.fn();
    await mount(ManageListsDialog, { virtualLibraries: { Sci: "tags:scifi" }, savedSearches: { Recent: "date:>1d" }, query: "author:lee", onChanged: changed });
    expect(dialog()!.textContent).toContain("Sci");
    expect(dialog()!.textContent).toContain("Recent");
    const inputs = document.body.querySelectorAll<HTMLInputElement>(".manage-form input");
    expect(inputs[1].value).toBe("author:lee");
    inputs[0].value = "Lee";
    inputs[0].dispatchEvent(new Event("input"));
    document.body.querySelector<HTMLFormElement>(".manage-form")!.dispatchEvent(new Event("submit", { cancelable: true }));
    await settle();
    expect(api.setVirtualLibrary).toHaveBeenCalledWith("Lee", "author:lee");
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("Custom columns: loads the columns when it opens", async () => {
    api.fetchCustomColumns.mockResolvedValue({ shelf: { name: "Shelf", datatype: "text" } });
    await mount(CustomColumnsDialog, {});
    expect(dialog()!.textContent).toContain("Shelf");
    expect(dialog()!.textContent).toContain("#shelf");
  });

  it("Check library: stays mounted while closed, and runs the check when it opens", async () => {
    api.checkLibrary.mockResolvedValue({ conclusive: true, needs_attention: false });
    api.fetchOrphans.mockResolvedValue([]);
    api.fetchIgnored.mockResolvedValue([]);
    host = document.createElement("div");
    document.body.appendChild(host);
    const open = ref(false);
    app = createApp({ setup: () => () => h(CheckLibraryDialog, { open: open.value }) });
    app.mount(host);
    await settle();
    expect(dialog()).toBeNull();
    expect(api.checkLibrary).not.toHaveBeenCalled();
    open.value = true;
    await settle();
    expect(dialog()).not.toBeNull();
    expect(api.checkLibrary).toHaveBeenCalledTimes(1);
    expect(dialog()!.textContent).toContain("No problems found.");
  });

  it("Fetch news: stays mounted while closed; opening shows the form with Fetch once as the primary", async () => {
    api.listNewsSchedules.mockResolvedValue({ schedules: [] });
    const open = ref(false);
    host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ setup: () => () => h(NewsDialog, { open: open.value }) });
    app.mount(host);
    await settle();
    expect(dialog()).toBeNull();
    open.value = true;
    await settle();
    expect(labels()).toEqual(["Save schedule…", "Close", "Fetch once"]);
    expect(footerButtons()[2].classList.contains("primary")).toBe(true);
    expect(api.listNewsSchedules).toHaveBeenCalledTimes(1);
  });

  it("Find duplicates: renders the review inside a full-size dialog when open", async () => {
    const store = createDuplicatesStore({ fetchDuplicates: async () => ({ groups: [], total: 0 }) } as never);
    const open = ref(true);
    host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ setup: () => () => h(DuplicatesDialog, { open: open.value, store, mergeNote: "Merged 2 books." }) });
    app.mount(host);
    await settle();
    expect(dialog()).not.toBeNull();
    expect(dialog()!.textContent).toContain("Merged 2 books.");
    expect(dialog()!.textContent).toContain("Analyse library");
    expect(dialog()!.textContent).not.toContain("Fingerprint PDFs");
    open.value = false;
    await settle();
    expect(dialog()).toBeNull();
  });

  it("the bulk edit panel is driven by its controller", async () => {
    const run = vi.fn();
    const bulk = reactive({
      open: true,
      busy: false,
      errors: ["Book 3: locked"],
      progress: null as { done: number; total: number } | null,
      spec: emptySpec(),
      applyRating: false,
      rating: 0,
      applySeriesIndex: false,
      seriesIndex: 1,
      summary: "Updated 1 book(s), 1 failed.",
      customColumns: [],
      run,
      cancel: vi.fn(),
    }) as unknown as BulkEdit;
    await mount(BulkEditPanel, { bulk, count: 3 });
    const text = host!.textContent!;
    expect(text).toContain("Apply to 3 book(s)");
    expect(text).toContain("Updated 1 book(s), 1 failed.");
    expect(text).toContain("Book 3: locked");
    host!.querySelector<HTMLButtonElement>("button.primary")!.click();
    expect(run).toHaveBeenCalledTimes(1);
    const tagsMode = host!.querySelector<HTMLSelectElement>(".bulk-field select")!;
    tagsMode.value = "remove";
    tagsMode.dispatchEvent(new Event("change"));
    expect(bulk.spec.tags!.mode).toBe("remove");
    Array.from(host!.querySelectorAll<HTMLButtonElement>("button")).find((b) => b.textContent === "Close")!.click();
    expect(bulk.open).toBe(false);
  });
});
