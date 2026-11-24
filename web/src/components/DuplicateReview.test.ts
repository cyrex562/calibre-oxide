// The group card's one-click "Keep this, trash N others": the label, which copy
// it keeps, the Undo toast it leaves, and what it does when the server refuses.

import { createApp, nextTick, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DuplicateGroup } from "../library/api";
import { dismissToast, toastState } from "../library/deleteFlow";
import { createDuplicatesStore } from "../library/duplicatesStore";
import DuplicateReview from "./DuplicateReview.vue";

const dedupe = vi.hoisted(() => ({ keepAndTrashGroup: vi.fn() }));
vi.mock("../library/dedupe", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/dedupe")>()), keepAndTrashGroup: dedupe.keepAndTrashGroup }));

let app: App | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  dedupe.keepAndTrashGroup.mockReset();
  dismissToast();
});
afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

const group = (over: Partial<DuplicateGroup> = {}): DuplicateGroup => ({
  id: "g1",
  reason: "content",
  confidence: 1,
  suggested_dest: 2,
  keeper_reason: "oldest record",
  books: [
    { book_id: 1, title: "A Very Long Title ".repeat(8), authors: ["x"] },
    { book_id: 2, title: "Dune", authors: ["x"] },
    { book_id: 3, title: "Dune again", authors: ["x"] },
  ],
  ...over,
});

async function mount(g: DuplicateGroup) {
  const store = createDuplicatesStore({} as never);
  store.state.hasScan = true;
  store.state.groups = [g];
  store.state.total = 1;
  store.state.allTotal = 1;
  const resolved = vi.spyOn(store, "resolved").mockResolvedValue();
  const changed = vi.fn();
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(DuplicateReview, { store, onChanged: changed });
  app.mount(host);
  await nextTick();
  return { store, resolved, changed };
}

const button = () => host!.querySelector<HTMLButtonElement>('[data-testid="keep-and-trash"]')!;

describe("Keep this, trash the others", () => {
  it("is the group's primary button and says how many copies go", async () => {
    await mount(group());
    expect(button().textContent!.trim()).toBe("Keep this, trash 2 others");
    expect(button().classList.contains("primary")).toBe(true);
    // Merge is still there, but it is not the primary action.
    expect(host!.querySelector('[data-testid="merge"]')!.classList.contains("primary")).toBe(false);
  });

  it("keeps the suggested copy, takes the group off the list and leaves an Undo toast", async () => {
    dedupe.keepAndTrashGroup.mockResolvedValue({ runId: "run-9", trashedIds: [1, 3] });
    const { resolved, changed } = await mount(group());
    button().click();
    await vi.waitFor(() => expect(resolved).toHaveBeenCalled());
    expect(dedupe.keepAndTrashGroup.mock.calls[0][1]).toBe(2);
    expect(resolved).toHaveBeenCalledWith(["g1"], [1, 3]);
    expect(changed).toHaveBeenCalled();
    expect(toastState.toast?.message).toBe("Kept “Dune” and moved 2 copies to Trash");
    expect(toastState.toast?.actionLabel).toBe("Undo");
  });

  it("keeps the copy the person chose instead of the suggestion", async () => {
    dedupe.keepAndTrashGroup.mockResolvedValue({ runId: "run-9", trashedIds: [2, 3] });
    const { store } = await mount(group());
    store.chooseKeeper(store.state.groups[0], 1);
    await nextTick();
    button().click();
    await vi.waitFor(() => expect(dedupe.keepAndTrashGroup).toHaveBeenCalled());
    expect(dedupe.keepAndTrashGroup.mock.calls[0][1]).toBe(1);
  });

  it("shows the server's reason and leaves the group where it is when it cannot be done", async () => {
    dedupe.keepAndTrashGroup.mockRejectedValue(new Error("The server's plan is not for exactly these books"));
    const { resolved } = await mount(group());
    button().click();
    await vi.waitFor(() => expect(host!.querySelector('[role="alert"]')?.textContent).toContain("not for exactly these books"));
    expect(resolved).not.toHaveBeenCalled();
    expect(toastState.toast).toBeNull();
  });

  it("is off for a group that is not in the last scan", async () => {
    await mount(group({ id: undefined, suggested_dest: 2 }));
    expect(button().disabled).toBe(true);
  });

  it("gives a long title its own lines (clamped, with the full title on hover) instead of drawing over the authors", async () => {
    await mount(group());
    const title = host!.querySelector<HTMLButtonElement>(".dup-info .title")!;
    expect(title.getAttribute("title")).toContain("A Very Long Title");
    expect(title.querySelector(".clamp")).not.toBeNull();
  });
});
