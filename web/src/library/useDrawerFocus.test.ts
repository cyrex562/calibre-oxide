// The narrow layout's drawers (C25): focus goes in, Escape closes, focus comes back.

import { afterEach, describe, expect, it } from "vitest";
import { createApp, defineComponent, h, nextTick, ref, type App } from "vue";

import type { Panel } from "./layout";
import { escapeClosesDrawer, useDrawerFocus } from "./useDrawerFocus";

let app: App | null = null;
afterEach(() => {
  app?.unmount();
  app = null;
  document.body.replaceChildren();
});

async function settle() {
  for (let i = 0; i < 3; i++) {
    await Promise.resolve();
    await nextTick();
  }
}

function mount() {
  const drawer = ref<Panel | null>(null);
  const host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(
    defineComponent({
      setup() {
        useDrawerFocus(drawer, () => (drawer.value = null));
        return () =>
          h("div", [
            h("button", { id: "toggle", onClick: () => (drawer.value = drawer.value ? null : "details") }, "Details"),
            drawer.value ? h("aside", { "data-drawer": "details", tabindex: -1, id: "panel" }, [h("input", { id: "field" })]) : null,
          ]);
      },
    }),
  );
  app.mount(host);
  return { drawer };
}

const press = (key: string) => window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

describe("a narrow drawer", () => {
  it("takes focus when it opens, closes on Escape and gives focus back to its opener", async () => {
    const { drawer } = mount();
    const toggle = document.getElementById("toggle")!;
    toggle.focus();
    toggle.click();
    await settle();
    expect(drawer.value).toBe("details");
    expect(document.activeElement).toBe(document.getElementById("panel"));

    press("Escape");
    await settle();
    expect(drawer.value).toBeNull();
    expect(document.activeElement).toBe(toggle);
  });

  it("leaves Escape alone when nothing is open", async () => {
    mount();
    const event = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });
});

describe("escapeClosesDrawer", () => {
  const esc = () => new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
  it("yields to an open dialog or menu, and to a key already handled", () => {
    expect(escapeClosesDrawer(esc())).toBe(true);
    const dialog = document.createElement("div");
    dialog.setAttribute("aria-modal", "true");
    document.body.appendChild(dialog);
    expect(escapeClosesDrawer(esc())).toBe(false);
    dialog.remove();
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    document.body.appendChild(menu);
    expect(escapeClosesDrawer(esc())).toBe(false);
    menu.remove();
    const handled = esc();
    handled.preventDefault();
    expect(escapeClosesDrawer(handled)).toBe(false);
    expect(escapeClosesDrawer(new KeyboardEvent("keydown", { key: "a" }))).toBe(false);
  });
});
