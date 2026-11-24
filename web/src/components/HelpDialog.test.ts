// Help (C20): a how-to whose buttons open the dialogs it talks about, and key notation that never
// reads two ways.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp, nextTick, type App } from "vue";

import { HELP_STEPS, helpActions } from "../library/help";
import { DEFAULT_KEYMAP } from "../settings/api";
import HelpDialog from "./HelpDialog.vue";

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
});

async function mount() {
  const run = vi.fn();
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(HelpDialog, { keymap: { ...DEFAULT_KEYMAP }, onRun: run });
  app.mount(host);
  await nextTick();
  return { run };
}

describe("HelpDialog", () => {
  it("explains import, duplicates, organize and export, with a button that opens each", async () => {
    const { run } = await mount();
    const text = document.body.querySelector(".dialog-body")!.textContent!;
    for (const step of HELP_STEPS) expect(text).toContain(step.title);
    const ids = Array.from(document.body.querySelectorAll<HTMLButtonElement>("[data-help-run]")).map((b) => b.dataset.helpRun);
    expect(ids).toEqual(expect.arrayContaining(["add-folder", "find-duplicates", "organize-files", "export-copy"]));
    document.body.querySelector<HTMLButtonElement>("[data-help-run=find-duplicates]")!.click();
    expect(run).toHaveBeenCalledWith("find-duplicates");
  });

  it("writes a plain key as typed and a capital as Shift+, so they never read alike", async () => {
    await mount();
    const kbds = Array.from(document.body.querySelectorAll("kbd")).map((k) => k.textContent);
    expect(kbds).toContain("d");
    expect(kbds).toContain("e");
    expect(kbds).toContain("Shift+E");
    expect(kbds).not.toContain("D");
    expect(kbds).toContain("→");
  });

  it("lists a label once, and narrows to what is typed", async () => {
    await mount();
    const labels = Array.from(document.body.querySelectorAll(".help-label")).map((l) => l.textContent);
    expect(labels.filter((l) => l === "Fix titles…").length).toBeLessThanOrEqual(1);
    const input = document.body.querySelector<HTMLInputElement>(".help-filter")!;
    input.value = "duplicates";
    input.dispatchEvent(new Event("input"));
    await nextTick();
    const shown = Array.from(document.body.querySelectorAll(".help-group .help-label")).map((l) => l.textContent!.toLowerCase());
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.every((l) => l.includes("duplicates"))).toBe(true);
  });
});

describe("helpActions", () => {
  it("shows each label once per group", () => {
    for (const group of ["library", "selection", "book", "view"] as const) {
      const labels = helpActions(group, { desktop: false }).map((a) => a.label);
      expect(new Set(labels).size).toBe(labels.length);
    }
  });
  it("leaves out desktop-only actions in a browser", () => {
    expect(helpActions("library", { desktop: false }).map((a) => a.id)).not.toContain("switch-library");
    expect(helpActions("library", { desktop: true }).map((a) => a.id)).toContain("switch-library");
  });
});
