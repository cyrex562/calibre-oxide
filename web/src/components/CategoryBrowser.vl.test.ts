// The category panel counts over the active virtual library: it asks the server
// for the categories with that library, and asks again when the library changes.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp, h, nextTick, ref, type App } from "vue";

import CategoryBrowser from "./CategoryBrowser.vue";
import { EXPANDED_KEY } from "../library/categoryTree";

const api = vi.hoisted(() => ({ fetchCategory: vi.fn() }));

vi.mock("../library/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../library/api")>()),
  fetchCategories: async () => [{ url: "/ajax/category/authors", name: "Authors", is_category: true }],
  fetchCategory: api.fetchCategory,
}));
vi.mock("./FoldersTree.vue", async () => ({ default: (await import("vue")).defineComponent({ setup: () => () => null }) }));

let app: App | null = null;

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem(EXPANDED_KEY, JSON.stringify(["authors"]));
  api.fetchCategory.mockReset();
  api.fetchCategory.mockImplementation(async (_key: string, _num: number, vl: string) => ({ items: [{ name: "Jane", count: vl ? 2 : 5, url: "/ajax/books_in/authors/1" }] }));
});

afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await nextTick();
  }
}

describe("the category panel under a virtual library", () => {
  it("asks for counts over the virtual library, and says nothing about the whole library", async () => {
    const vl = ref("Travel shelf");
    const host = document.createElement("div");
    document.body.appendChild(host);
    app = createApp({ setup: () => () => h(CategoryBrowser, { virtualLibrary: vl.value }) });
    app.mount(host);
    await settle();
    expect(api.fetchCategory).toHaveBeenLastCalledWith("authors", 200, "Travel shelf");
    expect(host.querySelector('[data-testid="vl-count-note"]')).toBeNull();
    expect(host.textContent).not.toContain("whole library");

    // Leaving the virtual library reloads the counts for the whole library.
    vl.value = "";
    await settle();
    expect(api.fetchCategory).toHaveBeenLastCalledWith("authors", 200, "");
  });
});
