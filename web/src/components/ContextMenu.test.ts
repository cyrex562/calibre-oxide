// Render tests for the right-click menu (issue 1.2 of the #816 epic).

import { createApp, type App } from "vue";
import { afterEach, describe, expect, it } from "vitest";

import ContextMenu, {} from "./ContextMenu.vue";
import { contextMenuEntries } from "../library/actions";

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

function mount(props: Record<string, unknown> = {}) {
  host = document.createElement("div");
  document.body.appendChild(host);
  const chosen: string[] = [];
  let closed = 0;
  app = createApp(ContextMenu, {
    x: 10,
    y: 10,
    entries: [
      { id: "read", label: "Read", enabled: true },
      { id: "convert", label: "Convert…", enabled: false },
      { id: "delete-book", label: "Delete", enabled: true, startsGroup: true },
    ],
    onChoose: (id: string) => chosen.push(id),
    onClose: () => (closed += 1),
    ...props,
  });
  app.mount(host);
  return { el: host, chosen, closed: () => closed };
}

describe("rendering", () => {
  it("renders every entry, enabled or not", () => {
    const { el } = mount();
    expect(el.querySelectorAll("button[role=menuitem]")).toHaveLength(3);
  });

  // A menu is also how someone discovers what is possible. An action
  // greyed out because nothing is selected teaches something; one
  // that vanishes teaches nothing.
  it("shows disabled entries greyed rather than hiding them", () => {
    const { el } = mount();
    const convert = [...el.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.includes("Convert"));
    expect(convert?.disabled).toBe(true);
  });

  it("draws a separator where a group starts", () => {
    expect(mount().el.querySelectorAll("hr")).toHaveLength(1);
  });

  it("says so when nothing is available", () => {
    const { el } = mount({ entries: [{ id: "read", label: "Read", enabled: false }] });
    expect(el.textContent).toContain("No actions available");
  });
});

describe("choosing", () => {
  it("reports the chosen action and closes", () => {
    const { el, chosen, closed } = mount();
    [...el.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.includes("Read"))?.click();
    expect(chosen).toEqual(["read"]);
    expect(closed()).toBe(1);
  });

  it("ignores a disabled entry", () => {
    const { el, chosen, closed } = mount();
    [...el.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.includes("Convert"))?.click();
    expect(chosen).toEqual([]);
    expect(closed()).toBe(0);
  });
});

describe("dismissal", () => {
  it("closes on Escape", () => {
    const { closed } = mount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(closed()).toBe(1);
  });

  // `pointerdown`, not `click`: the menu must be gone before the
  // click lands on whatever is underneath it.
  //
  // jsdom provides no `PointerEvent` constructor. The listener only
  // reads `event.target`, so a plain Event of the same type exercises
  // exactly the same path.
  it("closes when the pointer goes down elsewhere", () => {
    const { closed } = mount();
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(closed()).toBe(1);
  });

  it("stays open when the pointer goes down inside it", () => {
    const { el, closed } = mount();
    el.querySelector("button")?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(closed()).toBe(0);
  });
});

describe("entries from the registry", () => {
  it("offers the book actions for a single selection", () => {
    const entries = contextMenuEntries(["read", "convert", "delete-book"], { selectionCount: 1, isDesktop: false });
    expect(entries.map((e) => e.id)).toEqual(["read", "convert", "delete-book"]);
    expect(entries.every((e) => e.enabled)).toBe(true);
  });

  it("greys out single-book actions when several are selected", () => {
    const entries = contextMenuEntries(["read", "delete-book"], { selectionCount: 3, isDesktop: false });
    expect(entries.find((e) => e.id === "read")?.enabled).toBe(false);
    // Deleting several at once is a normal thing to want.
    expect(entries.find((e) => e.id === "delete-book")?.enabled).toBe(true);
  });

  it("omits desktop-only actions in a browser tab", () => {
    const ids = contextMenuEntries(["open-externally", "read"], { selectionCount: 1, isDesktop: false }).map((e) => e.id);
    expect(ids).toEqual(["read"]);
    const desktop = contextMenuEntries(["open-externally", "read"], { selectionCount: 1, isDesktop: true }).map((e) => e.id);
    expect(desktop).toContain("open-externally");
  });

  it("separates the book group from the selection group", () => {
    const entries = contextMenuEntries(["read", "bulk-edit"], { selectionCount: 1, isDesktop: false });
    expect(entries.find((e) => e.id === "bulk-edit")?.startsGroup).toBe(true);
    expect(entries[0].startsGroup).toBeUndefined();
  });

  it("never offers an action with no handler behind it", () => {
    expect(contextMenuEntries([], { selectionCount: 1, isDesktop: true })).toEqual([]);
  });
});
