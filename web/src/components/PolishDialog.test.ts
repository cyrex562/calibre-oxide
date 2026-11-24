// Polish (C14): the title says "1 book", Polish waits for an option, and a tick box sits inline.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp, nextTick, type App } from "vue";

import PolishDialog from "./PolishDialog.vue";

vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), polishBooks: vi.fn(async () => ({ books: [], changed: 0, failed: 0 })) }));

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
});

async function mount(bookIds: number[]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(PolishDialog, { bookIds });
  app.mount(host);
  await nextTick();
}

const polishButton = () => Array.from(document.body.querySelectorAll<HTMLButtonElement>(".dialog-footer button")).find((b) => b.textContent?.trim() === "Polish")!;

describe("PolishDialog", () => {
  it("names one book in the singular and several in the plural", async () => {
    await mount([7]);
    expect(document.body.querySelector(".dialog-title")!.textContent).toBe("Polish 1 book");
    app?.unmount();
    document.body.replaceChildren();
    await mount([7, 8]);
    expect(document.body.querySelector(".dialog-title")!.textContent).toBe("Polish 2 books");
  });

  it("stays disabled until an action is ticked", async () => {
    await mount([7]);
    expect(polishButton().disabled).toBe(true);
    expect(polishButton().title).toMatch(/at least one/i);
    document.body.querySelector<HTMLInputElement>(".polish-actions input[type=checkbox]")!.click();
    await nextTick();
    expect(polishButton().disabled).toBe(false);
  });

  it("puts each tick box in a .field row, which the stylesheet lays out inline", async () => {
    await mount([7]);
    const row = document.body.querySelector(".polish-actions label")!;
    expect(row.classList.contains("field")).toBe(true);
    expect(row.classList.contains("checkbox")).toBe(true);
  });
});
