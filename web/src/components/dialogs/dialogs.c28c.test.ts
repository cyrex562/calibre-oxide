// Round-2 usability fixes in the dialogs (tranche 28c): Check library's footer and "Show book",
// Manage lists' Close and explanation, the custom-column form, and Merge's "why" disclosure.

import { createApp, h, nextTick, ref, type App } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import CheckLibraryDialog from "./CheckLibraryDialog.vue";
import CustomColumnsDialog from "./CustomColumnsDialog.vue";
import ManageListsDialog from "./ManageListsDialog.vue";
import MergeBooksDialog from "./MergeBooksDialog.vue";

const api = vi.hoisted(() => ({
  checkLibrary: vi.fn(),
  cancelLibraryJob: vi.fn(async () => ({})),
  fetchOrphans: vi.fn(),
  fetchIgnored: vi.fn(),
  fetchCustomColumns: vi.fn(),
  addCustomColumn: vi.fn(),
}));
vi.mock("../../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../library/api")>()), ...api }));

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
  for (const m of Object.values(api)) m.mockReset();
});

async function settle() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

async function mount(component: object, props: Record<string, unknown>) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(component, props);
  app.mount(host);
  await settle();
}

const footer = () => Array.from(document.body.querySelectorAll<HTMLButtonElement>(".dialog-footer button"));
const labels = () => footer().map((b) => b.textContent!.trim());

describe("Check library", () => {
  const result = {
    conclusive: true,
    missing_files: [{ book_id: 30, title: "The Algorithms of Theory", path: "Fiction/Fantasy/scan0030.pdf" }],
    untracked_files: [{ book_id: null, title: "", path: "Tech/stray notes.txt" }],
  };

  it("ends with Check again and Close, and Check again runs the check once more", async () => {
    api.checkLibrary.mockResolvedValue(result);
    api.fetchOrphans.mockResolvedValue([]);
    api.fetchIgnored.mockResolvedValue([]);
    await mount(CheckLibraryDialog, { open: true });
    expect(labels()).toEqual(["Check again", "Close"]);
    expect(api.checkLibrary).toHaveBeenCalledTimes(1);

    api.checkLibrary.mockResolvedValue({ conclusive: true });
    footer()[0].click();
    await settle();
    expect(api.checkLibrary).toHaveBeenCalledTimes(2);
    expect(document.body.querySelector("[role=dialog]")!.textContent).toContain("No problems found.");
  });

  it("offers Show book for a finding about a book, and names a file by its file name", async () => {
    api.checkLibrary.mockResolvedValue(result);
    api.fetchOrphans.mockResolvedValue([]);
    api.fetchIgnored.mockResolvedValue([]);
    const shown = vi.fn();
    await mount(CheckLibraryDialog, { open: true, onShowBook: shown });
    const buttons = document.body.querySelectorAll<HTMLButtonElement>("[data-action=show-book]");
    expect(buttons).toHaveLength(1); // the untracked file has no book
    buttons[0].click();
    expect(shown).toHaveBeenCalledWith(30);
    expect(document.body.textContent).toContain("stray notes.txt");
    expect(document.body.textContent).not.toMatch(/—\s*Tech\/stray/);
  });
});

describe("Check library as a job", () => {
  /** A check the test finishes by hand. */
  function pendingCheck() {
    let finish: (v: unknown) => void = () => {};
    let progress: ((job: unknown) => void) | undefined;
    let started: ((id: number) => void) | undefined;
    api.checkLibrary.mockImplementation((handlers: { onStart?: (id: number) => void; onProgress?: (job: unknown) => void }) => {
      started = handlers.onStart;
      progress = handlers.onProgress;
      return new Promise((resolve) => (finish = resolve));
    });
    return { finish: (v: unknown) => finish(v), progress: (job: unknown) => progress?.(job), started: (id: number) => started?.(id) };
  }

  it("shows how far the check has got, and Cancel stops that job", async () => {
    api.fetchOrphans.mockResolvedValue([]);
    api.fetchIgnored.mockResolvedValue([]);
    const check = pendingCheck();
    await mount(CheckLibraryDialog, { open: true });
    expect(document.body.querySelector("[data-testid=check-progress]")!.textContent).toContain("Scanning");

    check.started(41);
    check.progress({ progress: { phase: "walking", seen: 12345, total: 0 } });
    await settle();
    expect(document.body.querySelector("[data-testid=check-progress]")!.textContent).toContain("12,345 files so far");

    document.body.querySelector<HTMLButtonElement>("[data-testid=check-cancel]")!.click();
    await settle();
    expect(api.cancelLibraryJob).toHaveBeenCalledWith(41);

    check.finish({ cancelled: true, conclusive: false });
    await settle();
    expect(document.body.querySelector("[role=dialog]")!.textContent).toContain("The check was stopped");
    expect(document.body.querySelector("[data-testid=check-progress]")).toBeNull();
  });

  it("opening the dialog again while a check runs follows it instead of starting another", async () => {
    api.fetchOrphans.mockResolvedValue([]);
    api.fetchIgnored.mockResolvedValue([]);
    const check = pendingCheck();
    const open = ref(true);
    const host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ setup: () => () => h(CheckLibraryDialog, { open: open.value }) });
    app.mount(host);
    await settle();
    expect(api.checkLibrary).toHaveBeenCalledTimes(1);

    open.value = false;
    await settle();
    open.value = true;
    await settle();
    expect(api.checkLibrary).toHaveBeenCalledTimes(1);
    expect(document.body.querySelector("[data-testid=check-progress]")).not.toBeNull();

    check.finish({ conclusive: true });
    await settle();
    expect(document.body.querySelector("[role=dialog]")!.textContent).toContain("No problems found.");
  });
});

describe("Manage virtual libraries and saved searches", () => {
  it("explains the difference, and has a Close button", async () => {
    const closed = vi.fn();
    await mount(ManageListsDialog, { virtualLibraries: {}, savedSearches: {}, query: "", onClose: closed });
    expect(document.body.querySelector("[role=dialog]")!.textContent).toMatch(/virtual library limits everything/i);
    expect(labels()).toEqual(["Close"]);
    footer()[0].click();
    expect(closed).toHaveBeenCalled();
  });
});

describe("Custom columns", () => {
  it("asks for a display name and derives the label from it, until the label is edited by hand", async () => {
    api.fetchCustomColumns.mockResolvedValue({});
    api.addCustomColumn.mockResolvedValue(1);
    const changed = vi.fn();
    await mount(CustomColumnsDialog, { onChanged: changed });
    const name = document.body.querySelector<HTMLInputElement>("input[aria-label='Display name']")!;
    const label = document.body.querySelector<HTMLInputElement>("input[aria-label='Label']")!;
    name.value = "Scan Quality";
    name.dispatchEvent(new Event("input"));
    await settle();
    expect(label.value).toBe("scan_quality");

    document.body.querySelectorAll<HTMLFormElement>(".manage-form")[document.body.querySelectorAll(".manage-form").length - 1].dispatchEvent(new Event("submit", { cancelable: true }));
    await settle();
    expect(api.addCustomColumn).toHaveBeenCalledWith("scan_quality", "Scan Quality", "text");
    expect(changed).toHaveBeenCalledTimes(1);
  });
});

describe("Merge books", () => {
  it("explains the pre-selected record in a disclosure, not a bare line of text", async () => {
    const books = [
      { book_id: 1, title: "A", authors: ["x"], formats: 1, metadata: 1, has_cover: true },
      { book_id: 2, title: "B", authors: ["x"], formats: 1, metadata: 1, has_cover: true },
    ];
    const open = ref(true);
    const host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ setup: () => () => (open.value ? h(MergeBooksDialog, { books, dest: 1 }) : null) });
    app.mount(host);
    await settle();
    const details = document.body.querySelector("details")!;
    expect(details.querySelector("summary")!.textContent).toBe("Why is one pre-selected?");
    expect(details.textContent).toContain("most formats");
  });
});
