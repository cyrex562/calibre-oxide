// Render test for the compare view: the template over ../library/compare.ts,
// with non-PDF books (covers) so no PDF.js is involved.

import { createApp, nextTick, type App } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import CompareDuplicates from "./CompareDuplicates.vue";
import { createCompareController } from "../library/compare";
import type { CompareApi } from "../library/compare";
import type { CompareBook, CompareResult } from "../library/api";

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

const book = (id: number): CompareBook => ({
  book_id: id,
  title: `Book ${id}`,
  authors: ["Ann Author"],
  path: "Ann Author/Folder",
  size: 2048,
  format_list: ["EPUB"],
  cover_url: `/get/thumb/${id}`,
  files: [{ format: "EPUB", path: `Ann Author/Folder/Book ${id}.epub`, size: 2048, mtime: 1_700_000_000 }],
});

const result = (ids: number[]): CompareResult => ({
  ids,
  books: ids.map(book),
  pairs: [{ a: ids[0], b: ids[1], identical_bytes: true, identical_formats: ["EPUB"], exempt: false, pdf: null, evidence: [{ reason: "title_author_exact", confidence: 0.9, detail: "same title" }] }],
  suggested_keeper: ids[0],
  keeper_reason: "more formats",
  pdf: { books: 0, fingerprinted: 0 },
});

async function mount() {
  const api = {
    fetchCompare: vi.fn(async (ids: number[]) => result(ids)),
    setFields: vi.fn(async () => ({})),
    refreshDuplicateBooks: vi.fn(async () => ({ refreshed: 1, has_scan: true })),
  } as unknown as CompareApi;
  const controller = createCompareController(api);
  await controller.open([1, 2, 3]);
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(CompareDuplicates, { controller });
  app.mount(host);
  await nextTick();
  return { controller, api: api as unknown as Record<string, ReturnType<typeof vi.fn>> };
}

describe("the compare view", () => {
  it("shows a column per copy with its facts, a cover, and the pair evidence", async () => {
    await mount();
    expect(document.body.querySelectorAll('[data-testid="column"]')).toHaveLength(3);
    expect(document.body.querySelectorAll('[data-testid="cover-image"]')).toHaveLength(3);
    const facts = document.body.querySelector('[data-testid="facts"]')!.textContent!;
    expect(facts).toContain("Book 1.epub");
    expect(facts).toContain("Ann Author/Folder");
    expect(document.body.querySelector('[data-testid="evidence"]')!.textContent).toContain("Identical files");
    // No PDFs: no page control.
    expect(document.body.querySelector('[data-testid="page-first"]')).toBeNull();
  });

  it("marks the suggested copy and lets the person choose another", async () => {
    const { controller } = await mount();
    expect(document.body.querySelectorAll('[data-testid="keeper-mark"]')).toHaveLength(1);
    (document.body.querySelectorAll('[data-testid="keep-this"]')[0] as HTMLButtonElement).click();
    await nextTick();
    expect(controller.state.keeperId).toBe(2);
    expect(document.body.querySelector('[data-testid="keeper-mark"]')!.textContent).toContain("Your choice");
  });

  it("saves an edited title through the controller", async () => {
    const { controller, api } = await mount();
    const input = document.body.querySelector('[data-testid="edit-title"]') as HTMLInputElement;
    input.value = "New Title";
    input.dispatchEvent(new Event("input"));
    await nextTick();
    expect(controller.state.drafts[1].title).toBe("New Title");
    (document.body.querySelector('[data-testid="save-metadata"]') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(api.setFields).toHaveBeenCalledWith(1, { title: "New Title" }));
  });

  it("offers Save as my group until the set is one", async () => {
    const { controller } = await mount();
    expect(document.body.querySelector('[data-testid="save-group"]')).not.toBeNull();
    controller.state.manualGroupId = "mg-1";
    await nextTick();
    expect(document.body.querySelector('[data-testid="save-group"]')).toBeNull();
    expect(document.body.querySelector('[data-testid="your-group"]')).not.toBeNull();
  });
});
