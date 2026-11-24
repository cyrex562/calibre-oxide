// The dialogs' action buttons live in the Dialog's footer slot, so they sit in
// one right-aligned row at the foot of the frame whatever the body holds.

import { afterEach, describe, expect, it } from "vitest";
import { createApp, defineComponent, h, nextTick, ref, type App } from "vue";

import HelpDialog from "../HelpDialog.vue";
import ImportDuplicatesDialog from "./ImportDuplicatesDialog.vue";
import NewLibraryDialog from "./NewLibraryDialog.vue";
import SwitchLibraryDialog from "./SwitchLibraryDialog.vue";

const apps: { app: App; el: HTMLElement }[] = [];
afterEach(() => {
  while (apps.length) {
    const { app, el } = apps.pop()!;
    app.unmount();
    el.remove();
  }
  document.body.replaceChildren();
});

function mount(component: unknown, props: Record<string, unknown>) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = createApp(component as never, props);
  app.mount(el);
  apps.push({ app, el });
}

const footerButtons = () => [...document.body.querySelectorAll<HTMLButtonElement>(".dialog-footer > button")];
const labels = () => footerButtons().map((b) => b.textContent!.trim());

describe("dialog footers", () => {
  it("New library: Cancel and a primary submit that submits the name form", async () => {
    const submitted: string[] = [];
    const closed: number[] = [];
    const name = ref("Novels");
    mount(
      defineComponent({
        setup: () => () => h(NewLibraryDialog, { busy: false, error: null, name: name.value, "onUpdate:name": (v: string) => (name.value = v), onSubmit: () => submitted.push(name.value), onClose: () => closed.push(1) }),
      }),
      {},
    );
    await nextTick();
    expect(labels()).toEqual(["Cancel", "Choose folder…"]);
    const [cancel, go] = footerButtons();
    expect(go.classList.contains("primary")).toBe(true);
    // The submit button is outside the <form>; the `form` attribute ties it back.
    expect(go.form).toBe(document.getElementById("new-library-form"));
    go.click();
    expect(submitted).toEqual(["Novels"]);
    cancel.click();
    expect(closed).toEqual([1]);
  });

  it("New library: the submit is off while the name is empty or the dialog is busy", async () => {
    mount(NewLibraryDialog, { busy: false, error: null, name: "  " });
    await nextTick();
    expect(footerButtons()[1].disabled).toBe(true);
  });

  it("Switch library: Close and Browse are in the footer, not the body", async () => {
    mount(SwitchLibraryDialog, { recent: ["D:\\Books"], switching: false, error: null });
    await nextTick();
    expect(labels().sort()).toEqual(["Browse for another…", "Close"]);
    expect(document.body.querySelector(".dialog-body")!.textContent).not.toContain("Browse for another");
  });

  it("Import duplicates: the three choices are in the footer", async () => {
    mount(ImportDuplicatesDialog, { duplicates: [{ file: { name: "a.pdf" }, existing: ["A"], checked: true }], busy: false });
    await nextTick();
    expect(labels()).toEqual(["Skip all", "Add all", "Add 1 ticked"]);
  });

  it("Help: Close is the footer's primary button", async () => {
    mount(HelpDialog, { keymap: {} });
    await nextTick();
    expect(labels()).toEqual(["Close"]);
    expect(footerButtons()[0].classList.contains("primary")).toBe(true);
  });
});
