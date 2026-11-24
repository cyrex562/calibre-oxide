// "Add folder" says what layout the copies get, and sends the choice.

import { createApp, nextTick, type App } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import ImportFolderDialog from "./ImportFolderDialog.vue";

const api = vi.hoisted(() => ({ importFolder: vi.fn() }));

vi.mock("../library/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../library/api")>()),
  importFolder: api.importFolder,
  fetchAbsorbable: async () => ({ libraries: [], conclusive: true }),
}));

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  api.importFolder.mockReset();
});

async function settle() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

async function mount() {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(ImportFolderDialog, { initialSource: "D:\\Books" });
  app.mount(host);
  await settle();
}

describe("the Add folder dialog's layout choice", () => {
  it("explains calibre's layout by default and keeps the source folders when asked", async () => {
    api.importFolder.mockResolvedValue({ files_seen: 0, books_added: 0, formats_added_to_existing: 0, skipped_same_format: [], failed: [], walk_errors: [], cancelled: false, conclusive: true });
    await mount();
    const help = () => document.body.querySelector("[data-testid=layout-help]")!.textContent!;
    expect(help()).toContain("Author/Title (id)");
    expect(help()).toContain("not kept");

    const select = document.body.querySelector<HTMLSelectElement>("[data-testid=layout]")!;
    select.selectedIndex = 1;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(help()).toContain("same place under the library");

    document.body.querySelector<HTMLButtonElement>("button.primary")!.click();
    await settle();
    expect(api.importFolder).toHaveBeenCalledTimes(1);
    expect(api.importFolder.mock.calls[0][0]).toMatchObject({ source: "D:\\Books", keep_structure: true });
  });

  it("sends calibre's layout when the choice is left alone", async () => {
    api.importFolder.mockResolvedValue({ files_seen: 0, books_added: 0, formats_added_to_existing: 0, skipped_same_format: [], failed: [], walk_errors: [], cancelled: false, conclusive: true });
    await mount();
    document.body.querySelector<HTMLButtonElement>("button.primary")!.click();
    await settle();
    expect(api.importFolder.mock.calls[0][0]).toMatchObject({ keep_structure: false });
  });
});
