import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ deleteBooks: vi.fn(), restoreDeletedBooks: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<typeof import("./api")>()), ...api }));

import { answerTrashConfirm, confirmState, describeTrashConfirm, dismissToast, movedToTrashMessage, runToastAction, toastState, trashBooksFlow, trashQuestion } from "./deleteFlow";

const items = (n: number) => Array.from({ length: n }, (_, i) => ({ id: i + 1, title: `Book ${i + 1}` }));

beforeEach(() => {
  api.deleteBooks.mockReset();
  api.restoreDeletedBooks.mockReset();
  dismissToast();
});
afterEach(() => {
  answerTrashConfirm(false);
  dismissToast();
});

describe("the confirm's wording", () => {
  it("is one question everywhere: move N books to the library Trash", () => {
    expect(trashQuestion(1)).toBe("Move 1 book to the library Trash?");
    expect(trashQuestion(12)).toBe("Move 12 books to the library Trash?");
    expect(trashQuestion(1200)).toBe("Move 1,200 books to the library Trash?");
    expect(movedToTrashMessage(3)).toBe("Moved 3 books to Trash");
    expect(`${trashQuestion(3)} ${describeTrashConfirm(items(3)).note}`).not.toMatch(/cannot be undone/);
  });

  it("names up to five titles and counts the rest", () => {
    const t = describeTrashConfirm(items(8));
    expect(t.titles).toEqual(["Book 1", "Book 2", "Book 3", "Book 4", "Book 5"]);
    expect(t.moreCount).toBe(3);
    expect(describeTrashConfirm(items(2)).moreCount).toBe(0);
  });

  it("says how many are hidden by the current search, only when some are", () => {
    expect(describeTrashConfirm(items(4), 0).hiddenNote).toBeNull();
    expect(describeTrashConfirm(items(4), 3).hiddenNote).toBe("3 of these are hidden by the current search.");
    expect(describeTrashConfirm(items(4), 1).hiddenNote).toBe("1 of these is hidden by the current search.");
  });
});

describe("trashBooksFlow", () => {
  it("does not delete anything when the confirm is declined", async () => {
    const flow = trashBooksFlow(items(2));
    expect(confirmState.open).toBe(true);
    expect(confirmState.text?.titles).toEqual(["Book 1", "Book 2"]);
    answerTrashConfirm(false);
    expect((await flow).cancelled).toBe(true);
    expect(api.deleteBooks).not.toHaveBeenCalled();
    expect(toastState.toast).toBeNull();
  });

  it("deletes, shows Moved N books to Trash with Undo, and Undo restores exactly those books", async () => {
    api.deleteBooks.mockResolvedValue({ deleted: [1, 2], failed: [] });
    api.restoreDeletedBooks.mockResolvedValue(2);
    const onRestored = vi.fn();
    const flow = trashBooksFlow(items(2), { onRestored });
    answerTrashConfirm(true);
    const result = await flow;
    expect(api.deleteBooks).toHaveBeenCalledWith([1, 2]);
    expect(result.failureText).toBe("");
    expect(toastState.toast?.message).toBe("Moved 2 books to Trash");
    expect(toastState.toast?.actionLabel).toBe("Undo");

    await runToastAction();
    expect(api.restoreDeletedBooks).toHaveBeenCalledWith([1, 2]);
    expect(onRestored).toHaveBeenCalledTimes(1);
    expect(toastState.toast?.result).toBe("Restored 2 books");
  });

  it("reports the books that could not be deleted and only offers Undo for the ones that were", async () => {
    api.deleteBooks.mockResolvedValue({ deleted: [1], failed: [{ book_id: 2, error: "in use" }] });
    const flow = trashBooksFlow(items(2));
    answerTrashConfirm(true);
    const result = await flow;
    expect(result.failureText).toContain("1 could not be deleted");
    expect(result.failureText).toContain("in use");
    expect(toastState.toast?.message).toBe("Moved 1 book to Trash");
    await runToastAction();
    expect(api.restoreDeletedBooks).toHaveBeenCalledWith([1]);
  });

  it("shows no Undo when nothing was deleted", async () => {
    api.deleteBooks.mockResolvedValue({ deleted: [], failed: [{ book_id: 1, error: "nope" }] });
    const flow = trashBooksFlow(items(1));
    answerTrashConfirm(true);
    expect((await flow).failureText).toContain("nope");
    expect(toastState.toast).toBeNull();
  });

  it("puts the warnings and the hidden note in the confirm", async () => {
    const flow = trashBooksFlow(items(2), { hiddenCount: 1, warnings: ["This is the copy suggested to keep."] });
    expect(confirmState.warnings).toEqual(["This is the copy suggested to keep."]);
    expect(confirmState.text?.hiddenNote).toContain("hidden by the current search");
    answerTrashConfirm(false);
    await flow;
  });

  it("lets the request's own failure through to the caller", async () => {
    api.deleteBooks.mockRejectedValue(new Error("server down"));
    const flow = trashBooksFlow(items(1));
    answerTrashConfirm(true);
    await expect(flow).rejects.toThrow("server down");
  });
});
