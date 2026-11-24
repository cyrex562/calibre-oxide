// The Appearance section of the settings view: it shows the saved skin and
// switches it at once, without waiting for the server's profiles to load.

import { createApp, nextTick, type App } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SKIN_KEY } from "../library/skin";
import SettingsView from "./SettingsView.vue";

let app: App | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-skin");
  // No server: every profile request fails, which must not hide the Appearance section.
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline"))),
  );
});
afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
  vi.unstubAllGlobals();
});

async function mount() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/", component: { template: "<div />" } }] });
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(SettingsView);
  app.use(router);
  app.mount(host);
  await nextTick();
  return host;
}

const radio = (el: HTMLElement, value: string) => el.querySelector<HTMLInputElement>(`input[data-skin-option="${value}"]`)!;

describe("Appearance", () => {
  it("offers Classic and Modern, with Classic selected by default", async () => {
    const el = await mount();
    expect(el.querySelector("#appearance-heading")!.textContent).toBe("Appearance");
    expect(radio(el, "classic").checked).toBe(true);
    expect(radio(el, "modern").checked).toBe(false);
  });

  it("switches the skin at once and remembers it", async () => {
    const el = await mount();
    radio(el, "modern").click();
    await nextTick();
    expect(document.documentElement.getAttribute("data-skin")).toBe("modern");
    expect(localStorage.getItem(SKIN_KEY)).toBe("modern");
    expect(radio(el, "modern").checked).toBe(true);

    radio(el, "classic").click();
    await nextTick();
    expect(document.documentElement.getAttribute("data-skin")).toBe("classic");
    expect(localStorage.getItem(SKIN_KEY)).toBe("classic");
  });

  it("shows a saved Modern choice when the view opens", async () => {
    localStorage.setItem(SKIN_KEY, "modern");
    const el = await mount();
    expect(radio(el, "modern").checked).toBe(true);
  });
});
