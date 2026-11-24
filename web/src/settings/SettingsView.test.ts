// The Appearance section of the settings view: it shows the saved skin and
// switches it at once, without waiting for the server's profiles to load.
// And the page's one Save: what counts as unsaved, what Save sends, and the nav.

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

/** A server that holds the profiles and records what is saved. */
function serverWithProfiles() {
  const saved: { name: string; profile: unknown }[] = [];
  const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
    const json = (body: unknown) => ({ ok: true, status: 200, statusText: "OK", json: async () => body, text: async () => JSON.stringify(body) });
    if (url.startsWith("/reader-profiles/get-all")) return json({});
    if (url.startsWith("/reader-profiles/save")) {
      saved.push(JSON.parse(String(init?.body)));
      return json(true);
    }
    if (url.startsWith("/email-account")) return json({ account: null, ok: true });
    return { ok: false, status: 503, statusText: "Service Unavailable", json: async () => ({}), text: async () => "" };
  });
  vi.stubGlobal("fetch", fetchMock);
  return { saved, fetchMock };
}

const settle = async () => {
  for (let i = 0; i < 8; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await nextTick();
  }
};

describe("the one Save", () => {
  it("has no unsaved changes at first, and lists the sections after an edit", async () => {
    serverWithProfiles();
    const el = await mount();
    await settle();
    const status = () => el.querySelector('[data-testid="save-status"]')!.textContent;
    const saveBtn = () => el.querySelector<HTMLButtonElement>('[data-testid="save-all"]')!;
    expect(status()).toBe("All changes saved.");
    expect(saveBtn().disabled).toBe(true);

    const sort = el.querySelector<HTMLSelectElement>("#settings-library select")!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change"));
    await nextTick();
    expect(status()).toBe("1 unsaved change: Library");
    expect(saveBtn().disabled).toBe(false);
    expect(el.querySelector('[data-nav="library"] .unsaved-dot')).not.toBeNull();
    expect(el.querySelector('[data-nav="reading"] .unsaved-dot')).toBeNull();
  });

  it("saves only the sections that changed, then says so", async () => {
    const { saved } = serverWithProfiles();
    const el = await mount();
    await settle();
    const sort = el.querySelector<HTMLSelectElement>("#settings-library select")!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change"));
    const theme = el.querySelector<HTMLSelectElement>("#settings-reading select")!;
    theme.value = "sepia";
    theme.dispatchEvent(new Event("change"));
    await nextTick();
    expect(el.querySelector('[data-testid="save-status"]')!.textContent).toBe("2 unsaved changes: Library, Reading");

    el.querySelector<HTMLButtonElement>('[data-testid="save-all"]')!.click();
    await settle();
    expect(saved.map((s) => s.name).sort()).toEqual(["library-prefs", "reader-prefs"]);
    expect(el.querySelector('[data-testid="save-status"]')!.textContent).toBe("Saved Library, Reading.");
    expect(el.querySelector<HTMLButtonElement>('[data-testid="save-all"]')!.disabled).toBe(true);
  });

  it("has one Save for the page, not one per section", async () => {
    serverWithProfiles();
    const el = await mount();
    await settle();
    const labels = Array.from(el.querySelectorAll("button")).map((b) => b.textContent!.trim());
    expect(labels.filter((l) => /^Save /.test(l))).toEqual(["Save changes"]);
  });

  it("lists the sections in a nav and leaves out the ones the server does not offer", async () => {
    serverWithProfiles();
    const el = await mount();
    await settle();
    const ids = Array.from(el.querySelectorAll("[data-nav]")).map((b) => b.getAttribute("data-nav"));
    expect(ids).toEqual(["appearance", "library", "typing", "reading", "keyboard", "toolbar", "colors", "email"]);
    expect(el.querySelector("#settings-plugins")).toBeNull();
  });
});
