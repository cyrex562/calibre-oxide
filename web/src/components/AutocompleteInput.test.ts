import { createApp, h, nextTick, ref, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AutocompleteInput from "./AutocompleteInput.vue";
import { invalidateFieldValues } from "../library/fieldValues";

vi.mock("../library/api", () => ({
  fetchCategoryValues: vi.fn(async () => ({
    items: [
      { name: "Jane Doe", count: 5 },
      { name: "John Smith", count: 3 },
      { name: "Jim Beam", count: 1 },
    ],
  })),
}));

let app: App | null = null;
let host: HTMLElement | null = null;
const text = ref("");
const outer = vi.fn();

function mount(props: Record<string, unknown> = {}) {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp({
    render: () =>
      h(AutocompleteInput, {
        modelValue: text.value,
        "onUpdate:modelValue": (x: string) => (text.value = x),
        field: "authors",
        separator: "&",
        class: "cell-editor",
        onKeydown: outer,
        ...props,
      }),
  });
  app.mount(host);
  return host.querySelector("input")!;
}

const flush = async () => {
  for (let i = 0; i < 4; i++) await Promise.resolve();
  await nextTick();
  await nextTick();
};

async function type(input: HTMLInputElement, value: string) {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event("input"));
  await flush();
}

function key(input: HTMLInputElement, k: string, init: KeyboardEventInit = {}) {
  const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
  const parentSpy = vi.fn();
  host!.addEventListener("keydown", parentSpy);
  input.dispatchEvent(e);
  host!.removeEventListener("keydown", parentSpy);
  return { e, reachedParent: parentSpy.mock.calls.length > 0 };
}

const rows = () => [...document.body.querySelectorAll<HTMLElement>('[role="option"]')];
const listOpen = () => document.body.querySelector<HTMLElement>('[role="listbox"]')!.style.display !== "none";

beforeEach(() => {
  text.value = "";
  outer.mockClear();
  invalidateFieldValues();
  localStorage.clear();
});
afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

describe("AutocompleteInput", () => {
  it("opens a list when typing enough characters, with counts, and passes attributes through", async () => {
    const input = mount();
    expect(input.classList.contains("cell-editor")).toBe(true);
    await type(input, "j");
    expect(listOpen()).toBe(false);
    await type(input, "jo");
    expect(listOpen()).toBe(true);
    expect(rows().map((r) => r.textContent)).toEqual(["John Smith3"]);
    expect(rows()[0].querySelector("mark")!.textContent).toBe("Jo");
    expect(rows()[0].querySelector(".ac-count")!.textContent).toBe("3");
  });

  it("exposes combobox/listbox aria", async () => {
    const input = mount();
    expect(input.getAttribute("role")).toBe("combobox");
    expect(input.getAttribute("aria-expanded")).toBe("false");
    await type(input, "ji");
    expect(input.getAttribute("aria-expanded")).toBe("true");
    const list = document.body.querySelector('[role="listbox"]')!;
    expect(input.getAttribute("aria-controls")).toBe(list.id);
    expect(input.getAttribute("aria-activedescendant")).toBeNull();
    key(input, "ArrowDown");
    await flush();
    expect(input.getAttribute("aria-activedescendant")).toBe(rows()[0].id);
    expect(rows()[0].getAttribute("aria-selected")).toBe("true");
  });

  it("Down then Enter accepts and consumes the key", async () => {
    const input = mount();
    await type(input, "Jane Doe & jo");
    key(input, "ArrowDown");
    await flush();
    const r = key(input, "Enter");
    await flush();
    expect(text.value).toBe("Jane Doe & John Smith");
    expect(r.e.defaultPrevented).toBe(true);
    expect(r.reachedParent).toBe(false);
    expect(outer).not.toHaveBeenCalled();
    expect(listOpen()).toBe(false);
  });

  it("Tab accepts the highlighted item and does not move on", async () => {
    const input = mount();
    await type(input, "ji");
    key(input, "ArrowDown");
    await flush();
    const r = key(input, "Tab");
    await flush();
    expect(text.value).toBe("Jim Beam");
    expect(r.e.defaultPrevented).toBe(true);
    expect(outer).not.toHaveBeenCalled();
  });

  it("Enter and Tab pass through when nothing is highlighted", async () => {
    const input = mount();
    await type(input, "jo");
    expect(listOpen()).toBe(true);
    const r = key(input, "Enter");
    expect(r.e.defaultPrevented).toBe(false);
    expect(r.reachedParent).toBe(true);
    expect(outer).toHaveBeenCalledTimes(1);
    key(input, "Tab");
    expect(outer).toHaveBeenCalledTimes(2);
    expect(text.value).toBe("jo");
  });

  it("Enter passes through when the list is closed", async () => {
    const input = mount();
    await type(input, "zzz");
    key(input, "Enter");
    expect(outer).toHaveBeenCalledTimes(1);
  });

  it("Esc closes the list, then the next Esc passes through", async () => {
    const input = mount();
    await type(input, "jo");
    const first = key(input, "Escape");
    await flush();
    expect(listOpen()).toBe(false);
    expect(first.reachedParent).toBe(false);
    expect(outer).not.toHaveBeenCalled();
    key(input, "Escape");
    expect(outer).toHaveBeenCalledTimes(1);
  });

  it("Down reopens a closed list and highlights the first row", async () => {
    const input = mount();
    await type(input, "ja");
    key(input, "Escape");
    await flush();
    expect(listOpen()).toBe(false);
    key(input, "ArrowDown");
    await flush();
    expect(listOpen()).toBe(true);
    expect(rows()[0].getAttribute("aria-selected")).toBe("true");
  });

  it("a mouse click accepts without blurring the input", async () => {
    const input = mount();
    await type(input, "Jane Doe & ji");
    const blur = vi.fn();
    input.addEventListener("blur", blur);
    const e = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    rows()[0].dispatchEvent(e);
    await flush();
    expect(e.defaultPrevented).toBe(true);
    expect(blur).not.toHaveBeenCalled();
    expect(text.value).toBe("Jane Doe & Jim Beam");
    expect(listOpen()).toBe(false);
  });

  it("uses a fixed list when given options", async () => {
    const input = mount({ field: undefined, separator: null, options: ["Draft", "Final"], minChars: 1 });
    await type(input, "d");
    expect(rows().map((r) => r.textContent)).toEqual(["Draft"]);
  });

  it("follows the minChars setting", async () => {
    localStorage.setItem("lexmechanic.suggestMinChars", "4");
    const input = mount();
    await type(input, "joh");
    expect(listOpen()).toBe(false);
    await type(input, "john");
    expect(listOpen()).toBe(true);
  });
});
