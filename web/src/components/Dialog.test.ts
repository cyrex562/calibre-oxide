import { afterEach, describe, expect, it } from "vitest";
import { createApp, defineComponent, h, nextTick, ref, type App, type Component } from "vue";

import { alertDialog, confirmDialog, promptDialog } from "./dialogs/confirm";
import Dialog from "./Dialog.vue";

interface Mounted {
  unmount(): void;
}
const mounted: Mounted[] = [];
afterEach(() => {
  while (mounted.length) mounted.pop()!.unmount();
  document.body.replaceChildren();
});

async function flushPromises() {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await nextTick();
  }
  await new Promise((r) => setTimeout(r, 0));
}

function mount(component: Component): Mounted {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app: App = createApp(component);
  app.mount(el);
  const handle: Mounted = {
    unmount() {
      app.unmount();
      el.remove();
    },
  };
  mounted.push(handle);
  return handle;
}

/** A host with an opener button and a dialog whose content is `inner`. */
function host(inner: () => unknown, props: Record<string, unknown> = {}) {
  const open = ref(false);
  const closed = ref(0);
  const wrapper = mount(
    defineComponent({
      setup: () => () =>
        h("div", [
          h("button", { id: "opener", onClick: () => (open.value = true) }, "open"),
          h(
            Dialog,
            { open: open.value, title: "Title", ...props, onClose: () => ((closed.value += 1), (open.value = false)) },
            { default: () => inner() as never, footer: () => h("button", { id: "ok", class: "primary" }, "OK") },
          ),
        ]),
    }),
  );
  return { wrapper, open, closed };
}

const panel = () => document.body.querySelector<HTMLElement>('[role="dialog"]');
const key = (k: string, init: KeyboardEventInit = {}, target: Element | Document = document.activeElement ?? document) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

describe("Dialog", () => {
  it("is a labelled modal dialog, with the footer slot", async () => {
    const { open } = host(() => h("input", { id: "a" }));
    expect(panel()).toBeNull();
    open.value = true;
    await nextTick();
    const el = panel()!;
    expect(el.getAttribute("aria-modal")).toBe("true");
    const labelled = el.getAttribute("aria-labelledby")!;
    expect(document.getElementById(labelled)!.textContent).toBe("Title");
    expect(el.querySelector(".dialog-footer button.primary")!.textContent).toBe("OK");
    // Teleported to <body>, outside the host's own tree.
    expect(el.closest("[data-dialog-backdrop]")!.parentElement).toBe(document.body);
  });

  it("uses `label` as the accessible name when it has no title", async () => {
    const { open } = host(() => h("input"), { title: undefined, label: "Help" });
    open.value = true;
    await nextTick();
    expect(panel()!.getAttribute("aria-label")).toBe("Help");
    expect(panel()!.hasAttribute("aria-labelledby")).toBe(false);
  });

  it("focuses [autofocus] first, else the first focusable, else the panel", async () => {
    const withAuto = host(() => [h("input", { id: "a" }), h("input", { id: "b", autofocus: true })]);
    withAuto.open.value = true;
    await flushPromises();
    expect(document.activeElement!.id).toBe("b");
    withAuto.wrapper.unmount();
    mounted.pop();

    const plain = host(() => [h("input", { id: "a" }), h("input", { id: "b" })]);
    plain.open.value = true;
    await flushPromises();
    expect(document.activeElement!.id).toBe("a");
    plain.wrapper.unmount();
    mounted.pop();

    // No footer and no focusable content: the panel itself takes focus.
    const open = ref(true);
    mount(defineComponent({ setup: () => () => h(Dialog, { open: open.value, title: "T" }, { default: () => h("p", "text") }) }));
    await flushPromises();
    expect(document.activeElement).toBe(panel());
  });

  it("keeps Tab and Shift+Tab inside the dialog", async () => {
    const { open } = host(() => [h("input", { id: "a" }), h("input", { id: "b" })]);
    open.value = true;
    await flushPromises();
    // Order: a, b, then the footer's #ok.
    const ok = document.getElementById("ok")!;
    ok.focus();
    const forward = key("Tab", {}, ok);
    expect(forward).toBe(false); // preventDefault was called
    expect(document.activeElement!.id).toBe("a");

    const back = key("Tab", { shiftKey: true }, document.activeElement!);
    expect(back).toBe(false);
    expect(document.activeElement!.id).toBe("ok");

    // Tab from the middle is left to the browser.
    document.getElementById("b")!.focus();
    expect(key("Tab", {}, document.activeElement!)).toBe(true);

    // Focus that strayed outside is pulled back in.
    (document.getElementById("opener") as HTMLElement).focus();
    key("Tab", {}, document.activeElement!);
    expect(panel()!.contains(document.activeElement)).toBe(true);
  });

  it("restores focus to the opener when it closes", async () => {
    const { open } = host(() => h("input", { id: "a" }));
    const opener = document.getElementById("opener")!;
    opener.focus();
    open.value = true;
    await flushPromises();
    expect(document.activeElement!.id).toBe("a");
    open.value = false;
    await flushPromises();
    expect(panel()).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("closes on Escape", async () => {
    const { open, closed } = host(() => h("input"));
    open.value = true;
    await flushPromises();
    key("Escape");
    await flushPromises();
    expect(closed.value).toBe(1);
    expect(panel()).toBeNull();
  });

  it("lets only the topmost of stacked dialogs handle Escape", async () => {
    const lower = ref(true);
    const upper = ref(true);
    let lowerClosed = 0;
    let upperClosed = 0;
    mount(
      defineComponent({
        setup: () => () =>
          h("div", [
            h(Dialog, { open: lower.value, title: "Lower", onClose: () => ((lowerClosed += 1), (lower.value = false)) }, { default: () => h("input", { id: "low" }) }),
            h(Dialog, { open: upper.value, title: "Upper", onClose: () => ((upperClosed += 1), (upper.value = false)) }, { default: () => h("input", { id: "up" }) }),
          ]),
      }),
    );
    await flushPromises();
    key("Escape");
    await flushPromises();
    expect(upperClosed).toBe(1);
    expect(lowerClosed).toBe(0);
    key("Escape");
    await flushPromises();
    expect(lowerClosed).toBe(1);
  });

  it("closes on a backdrop click, but not a click inside, and not while busy", async () => {
    const { open, closed } = host(() => h("input", { id: "a" }), { busy: true });
    open.value = true;
    await flushPromises();
    const backdrop = document.body.querySelector<HTMLElement>("[data-dialog-backdrop]")!;
    backdrop.click();
    await flushPromises();
    key("Escape");
    await flushPromises();
    expect(closed.value).toBe(0);
    expect(panel()!.getAttribute("aria-busy")).toBe("true");

    const free = host(() => h("input", { id: "b" }));
    free.open.value = true;
    await flushPromises();
    document.getElementById("b")!.click();
    await flushPromises();
    expect(free.closed.value).toBe(0);
    document.body.querySelectorAll<HTMLElement>("[data-dialog-backdrop]")[1].click();
    await flushPromises();
    expect(free.closed.value).toBe(1);
  });

  it("does not close when a text selection drag ends on the backdrop", async () => {
    const { open, closed } = host(() => h("input", { id: "a" }));
    open.value = true;
    await flushPromises();
    const backdrop = document.body.querySelector<HTMLElement>("[data-dialog-backdrop]")!;
    document.getElementById("a")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    backdrop.click();
    await flushPromises();
    expect(closed.value).toBe(0);
  });

  it("asks 'Discard changes?' before closing a dirty dialog", async () => {
    const { open, closed } = host(() => h("input", { id: "a" }), { dirty: true });
    open.value = true;
    await flushPromises();

    key("Escape");
    await flushPromises();
    const question = [...document.body.querySelectorAll('[role="dialog"]')].find((d) => d.textContent?.includes("Discard changes?"))!;
    expect(question).toBeTruthy();
    expect(closed.value).toBe(0);

    // Keep editing: the dialog stays.
    (document.body.querySelector('[data-testid="confirm-cancel"]') as HTMLElement).click();
    await flushPromises();
    expect(closed.value).toBe(0);
    expect(panel()).not.toBeNull();

    // A backdrop click asks again; confirming closes.
    document.body.querySelector<HTMLElement>("[data-dialog-backdrop]")!.click();
    await flushPromises();
    (document.body.querySelector('[data-testid="confirm-ok"]') as HTMLElement).click();
    await flushPromises();
    expect(closed.value).toBe(1);
  });

  it("closes a clean dialog without asking", async () => {
    const { open, closed } = host(() => h("input"), { dirty: false });
    open.value = true;
    await flushPromises();
    key("Escape");
    await flushPromises();
    expect(document.body.querySelector('[data-testid="confirm-dialog"]')).toBeNull();
    expect(closed.value).toBe(1);
  });
});

describe("confirmDialog / promptDialog / alertDialog", () => {
  it("resolves true on OK and false on Cancel or Escape", async () => {
    const yes = confirmDialog("Sure?");
    await flushPromises();
    expect(panel()!.textContent).toContain("Sure?");
    (document.body.querySelector('[data-testid="confirm-ok"]') as HTMLElement).click();
    expect(await yes).toBe(true);
    await flushPromises();
    expect(panel()).toBeNull();

    const no = confirmDialog("Sure?");
    await flushPromises();
    (document.body.querySelector('[data-testid="confirm-cancel"]') as HTMLElement).click();
    expect(await no).toBe(false);
    await flushPromises();

    const esc = confirmDialog("Sure?", { danger: true });
    await flushPromises();
    // A destructive confirm starts on Cancel, so a stray Enter does nothing harmful.
    expect(document.activeElement).toBe(document.body.querySelector('[data-testid="confirm-cancel"]'));
    key("Escape");
    expect(await esc).toBe(false);
  });

  it("prompts for text, with a starting value, and resolves null on cancel", async () => {
    const answer = promptDialog("Rename to:", "old");
    await flushPromises();
    const input = document.body.querySelector<HTMLInputElement>('[data-testid="confirm-input"]')!;
    expect(input.value).toBe("old");
    expect(document.activeElement).toBe(input);
    input.value = "new";
    input.dispatchEvent(new Event("input"));
    (document.body.querySelector('[data-testid="confirm-ok"]') as HTMLElement).click();
    expect(await answer).toBe("new");
    await flushPromises();

    const cancelled = promptDialog("Rename to:", "old");
    await flushPromises();
    key("Escape");
    expect(await cancelled).toBeNull();
  });

  it("alerts with a single button", async () => {
    const done = alertDialog("Heads up");
    await flushPromises();
    expect(document.body.querySelector('[data-testid="confirm-cancel"]')).toBeNull();
    (document.body.querySelector('[data-testid="confirm-ok"]') as HTMLElement).click();
    await done;
  });
});
