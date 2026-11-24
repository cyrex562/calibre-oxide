// The cover grid's keyboard and screen-reader behaviour (C24), row colours on its cards (C16)
// and the placeholder for a book with no cover (C21).

import { afterEach, describe, expect, it } from "vitest";
import { createApp, defineComponent, h, nextTick, ref, shallowReactive, type App } from "vue";

import type { BookSummary } from "../library/types";
import BookGrid from "./BookGrid.vue";

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
});

const books = new Map<number, BookSummary>();
for (let id = 1; id <= 6; id++) books.set(id, { id, title: `Book number ${id}`, authors: ["Ann Lee"], thumbnail: `/get/thumb/${id}` } as unknown as BookSummary);

async function settle() {
  for (let i = 0; i < 4; i++) {
    await Promise.resolve();
    await nextTick();
  }
}

async function mount(opts: { selectedBookId?: number | null; selectedIds?: number[]; rowColors?: Map<number, string> } = {}) {
  const navigated: [string, boolean][] = [];
  const selectedBookId = ref<number | null>(opts.selectedBookId ?? null);
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(
    defineComponent({
      setup: () => () =>
        h(BookGrid, {
          ids: [1, 2, 3, 4, 5, 6],
          getBook: (id: number) => books.get(id),
          selectMode: false,
          selectedIds: new Set(opts.selectedIds ?? []),
          selectedBookId: selectedBookId.value,
          markedIds: new Set<number>(),
          rowColors: opts.rowColors ?? shallowReactive(new Map<number, string>()),
          cacheBust: 0,
          onNavigate: (key: string, extend: boolean) => navigated.push([key, extend]),
        }),
    }),
  );
  app.mount(host);
  await settle();
  return { navigated, selectedBookId };
}

const cards = () => Array.from(document.body.querySelectorAll<HTMLElement>(".card[role=option]"));

describe("BookGrid", () => {
  it("is a multi-select listbox of options, and says which are selected", async () => {
    await mount({ selectedIds: [2, 3] });
    const list = document.body.querySelector("[role=listbox]")!;
    expect(list.getAttribute("aria-multiselectable")).toBe("true");
    expect(list.getAttribute("aria-label")).toBe("Books");
    expect(cards().map((c) => c.getAttribute("aria-selected"))).toEqual(["false", "true", "true", "false", "false", "false"]);
  });

  it("has one Tab stop: the open book's card, else the first", async () => {
    await mount({ selectedBookId: 4 });
    expect(cards().filter((c) => c.tabIndex === 0).map((c) => c.dataset.id)).toEqual(["4"]);
    app?.unmount();
    document.body.replaceChildren();
    await mount();
    expect(cards().filter((c) => c.tabIndex === 0).map((c) => c.dataset.id)).toEqual(["1"]);
  });

  it("asks to move on an arrow key, and with Shift extends", async () => {
    const { navigated } = await mount({ selectedBookId: 1 });
    const card = cards()[0];
    card.focus();
    card.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    card.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", shiftKey: true, bubbles: true, cancelable: true }));
    expect(navigated).toEqual([["ArrowRight", false], ["ArrowDown", true]]);
  });

  it("gives a long title a tooltip", async () => {
    await mount();
    expect(document.body.querySelector(".card-title")!.getAttribute("title")).toBe("Book number 1");
  });

  it("tints a coloured book's caption and leaves the others alone", async () => {
    await mount({ rowColors: new Map([[2, "blue"]]) });
    const colored = cards().filter((c) => c.classList.contains("row-colored"));
    expect(colored.map((c) => c.dataset.id)).toEqual(["2"]);
    expect(colored[0].style.getPropertyValue("--row-color")).toBe("blue");
  });

  it("draws a placeholder instead of a broken cover", async () => {
    await mount();
    const img = document.body.querySelector<HTMLImageElement>(".card img")!;
    img.dispatchEvent(new Event("error"));
    await settle();
    expect(cards()[0].querySelector(".no-cover")?.textContent).toBe("No cover");
    expect(cards()[0].querySelector("img")).toBeNull();
  });
});
