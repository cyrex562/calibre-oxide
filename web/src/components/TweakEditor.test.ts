// Tweak Book (C13): Escape and the ✕ ask before throwing away edits, the file list marks a changed
// file, and Save file / Save book wait for something to save.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp, nextTick, type App } from "vue";

import TweakEditor from "./TweakEditor.vue";

const tweak = vi.hoisted(() => ({
  openTweakSession: vi.fn(async () => ({ session_id: "s1", files: ["style.css", "text.xhtml"] })),
  fetchTweakFile: vi.fn(async () => "p { color: black; }"),
  saveTweakFile: vi.fn(async () => undefined),
  commitTweakSession: vi.fn(async () => undefined),
  discardTweakSession: vi.fn(async () => undefined),
  fetchToc: vi.fn(async () => []),
  saveToc: vi.fn(async () => undefined),
}));
vi.mock("../library/tweak", () => tweak);

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
  for (const fn of Object.values(tweak)) fn.mockClear();
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

async function open() {
  const closed = vi.fn();
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(TweakEditor, { bookId: 1, onClose: closed });
  app.mount(host);
  await settle();
  return { closed };
}

const footer = (label: string) => Array.from(document.body.querySelectorAll<HTMLButtonElement>(".dialog-footer button")).find((b) => b.textContent?.trim() === label)!;
const fileButton = (name: string) => Array.from(document.body.querySelectorAll<HTMLButtonElement>(".files-list button, .file-list button, li button")).find((b) => b.textContent?.trim().startsWith(name))!;
const confirmDialogs = () => document.body.querySelectorAll("[data-testid=confirm-dialog]");

async function editFile() {
  fileButton("style.css").click();
  await settle();
  const area = document.body.querySelector<HTMLTextAreaElement>("textarea")!;
  area.value = "p { color: red; }";
  area.dispatchEvent(new Event("input"));
  await settle();
  return area;
}

describe("Tweak Book with unsaved edits", () => {
  it("starts with nothing to save", async () => {
    await open();
    expect(footer("Save file").disabled).toBe(true);
    expect(footer("Save book").disabled).toBe(true);
  });

  it("asks before Escape throws the edits away, and stays open on Cancel", async () => {
    const { closed } = await open();
    await editFile();
    expect(footer("Save file").disabled).toBe(false);
    expect(footer("Save book").disabled).toBe(false);
    expect(fileButton("style.css").textContent).toContain("•");

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(confirmDialogs().length).toBe(1);
    expect(closed).not.toHaveBeenCalled();

    document.body.querySelector<HTMLButtonElement>("[data-testid=confirm-cancel]")!.click();
    await settle();
    expect(closed).not.toHaveBeenCalled();
    expect(tweak.discardTweakSession).not.toHaveBeenCalled();
  });

  it("closes and discards the session once the loss is confirmed", async () => {
    const { closed } = await open();
    await editFile();
    document.body.querySelector<HTMLButtonElement>(".manage-close")!.click();
    await settle();
    document.body.querySelector<HTMLButtonElement>("[data-testid=confirm-ok]")!.click();
    await settle();
    expect(closed).toHaveBeenCalledTimes(1);
    expect(tweak.discardTweakSession).toHaveBeenCalledWith("s1");
  });

  it("closes at once when nothing was changed", async () => {
    const { closed } = await open();
    fileButton("style.css").click();
    await settle();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(confirmDialogs().length).toBe(0);
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("keeps asking after a file was saved into the session, because the book is not saved yet", async () => {
    const { closed } = await open();
    await editFile();
    footer("Save file").click();
    await settle();
    expect(tweak.saveTweakFile).toHaveBeenCalled();
    expect(footer("Save file").disabled).toBe(true);
    expect(footer("Save book").disabled).toBe(false);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(confirmDialogs().length).toBe(1);
    expect(closed).not.toHaveBeenCalled();
  });
});
