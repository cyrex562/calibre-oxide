// The Settings nav's scroll-spy and the toolbar and keyboard lists (C22).

import { createApp, nextTick, type App } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SettingsView from "./SettingsView.vue";

let app: App | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  localStorage.clear();
  // jsdom draws nothing, so it does not scroll either.
  Element.prototype.scrollIntoView = vi.fn();
  const json = (body: unknown) => ({ ok: true, status: 200, statusText: "OK", json: async () => body, text: async () => JSON.stringify(body) });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      if (url.startsWith("/reader-profiles/get-all")) return json({});
      if (url.startsWith("/email-account")) return json({ account: null, ok: true });
      return { ok: false, status: 503, statusText: "Service Unavailable", json: async () => ({}), text: async () => "" };
    }),
  );
});
afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  vi.unstubAllGlobals();
});

async function settle() {
  for (let i = 0; i < 8; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
}

async function mount() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/", component: { template: "<div />" } }] });
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(SettingsView);
  app.use(router);
  app.mount(host);
  await settle();
  return host;
}

/** Lays the sections out as a page: tops are given by id, the others sit far below. */
function layOut(tops: Record<string, number>) {
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[id^='settings-']"))) {
    const id = el.id.replace("settings-", "");
    el.getBoundingClientRect = () => ({ top: tops[id] ?? 5000, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  }
}
const current = (el: HTMLElement) => Array.from(el.querySelectorAll("[data-nav].current")).map((b) => b.getAttribute("data-nav"));

describe("the nav's scroll-spy", () => {
  it("marks the section the page is in, and the one just clicked, not the last one on screen", async () => {
    const el = await mount();
    expect(current(el)).toEqual(["appearance"]);

    // Toolbar is at the top of the window with Email below it: Toolbar is the current one.
    layOut({ appearance: -2000, library: -1500, typing: -1200, reading: -900, keyboard: -500, toolbar: 20, colors: 300, email: 600 });
    window.dispatchEvent(new Event("scroll"));
    await nextTick();
    expect(current(el)).toEqual(["toolbar"]);

    // A click names its section, and a page too short to scroll all the way there does not hand the mark away.
    el.querySelector<HTMLElement>("[data-nav='colors']")!.click();
    layOut({ appearance: -3000, library: -2500, typing: -2200, reading: -1900, keyboard: -1500, toolbar: -1000, colors: -300, email: -10 });
    window.dispatchEvent(new Event("scroll"));
    await nextTick();
    expect(current(el)).toEqual(["colors"]);

    // Scrolling by hand takes over again.
    window.dispatchEvent(new Event("wheel"));
    window.dispatchEvent(new Event("scroll"));
    await nextTick();
    expect(current(el)).toEqual(["email"]);
  });
});

describe("the lists", () => {
  it("leave desktop-only actions out of the toolbar list in a browser", async () => {
    const el = await mount();
    const labels = Array.from(el.querySelectorAll("#settings-toolbar .toolbar-row label")).map((l) => l.textContent!.trim());
    expect(labels.length).toBeGreaterThan(0);
    expect(labels.some((l) => /desktop app only/.test(l))).toBe(false);
  });

  it("write keys the way the shortcut hints do", async () => {
    const el = await mount();
    const keys = Array.from(el.querySelectorAll("#settings-keyboard .keymap-row button")).map((b) => b.textContent!.trim());
    expect(keys).toContain("→");
    expect(keys).toContain("Shift+E");
    expect(keys).not.toContain("ArrowRight");
  });
});
