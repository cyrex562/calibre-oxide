// "Find it" / "Use this file" for a missing file, found by its checksum.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp, nextTick, type App } from "vue";

import LocateMissing from "./LocateMissing.vue";

const api = vi.hoisted(() => ({ locateOrphan: vi.fn() }));
vi.mock("../../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../library/api")>()), locateOrphan: api.locateOrphan }));

let app: App | null = null;
let host: HTMLElement;
const used: string[] = [];

beforeEach(() => {
  api.locateOrphan.mockReset();
  used.length = 0;
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(LocateMissing, { bookId: 7, format: "epub", onUse: (p: string) => used.push(p) });
  app.mount(host);
});

afterEach(() => {
  app?.unmount();
  host.remove();
  app = null;
});

async function settle() {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
    await nextTick();
  }
}
const buttons = () => [...host.querySelectorAll("button")].map((b) => b.textContent!.trim());
const click = async (label: string) => {
  [...host.querySelectorAll("button")].find((b) => b.textContent!.trim() === label)!.click();
  await settle();
};

describe("LocateMissing", () => {
  it("offers Find it first, and the matches as Use this file once found", async () => {
    api.locateOrphan.mockResolvedValue({ has_checksum: true, candidates: [{ path: "Moved/Book.epub" }, { path: "Copy/Book.epub" }], checked: 12 });
    expect(buttons()).toEqual(["Find it"]);
    await click("Find it");
    expect(api.locateOrphan).toHaveBeenCalledWith(7, "epub");
    expect(host.textContent).toContain("Moved/Book.epub");
    expect(buttons()).toEqual(["Find it", "Use this file", "Use this file"]);

    host.querySelectorAll("button")[2].click();
    expect(used).toEqual(["Copy/Book.epub"]);
  });

  it("says so when no file has the same content", async () => {
    api.locateOrphan.mockResolvedValue({ has_checksum: true, candidates: [], checked: 3 });
    await click("Find it");
    expect(host.querySelector('[data-testid="locate-none"]')!.textContent).toContain("3 checked");
    expect(buttons()).toEqual(["Find it"]);
  });

  it("does not claim there is no match when the search stopped early", async () => {
    api.locateOrphan.mockResolvedValue({ has_checksum: true, candidates: [], checked: 3, truncated: true });
    await click("Find it");
    const note = host.querySelector('[data-testid="locate-none"]')!.textContent!;
    expect(note).toContain("stopped early");
    expect(note).not.toContain("No file with the same content");
  });

  it("says so when no checksum was recorded", async () => {
    api.locateOrphan.mockResolvedValue({ has_checksum: false, candidates: [], checked: 0 });
    await click("Find it");
    expect(host.querySelector('[data-testid="locate-no-checksum"]')).not.toBeNull();
  });

  it("shows a failed search", async () => {
    api.locateOrphan.mockRejectedValue(new Error("boom"));
    await click("Find it");
    expect(host.querySelector('[role="alert"]')!.textContent).toContain("boom");
  });
});
