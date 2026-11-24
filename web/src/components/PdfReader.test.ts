// The PDF reader against a mocked pdf.js: the render race (C1) and the
// reader features that sit on top of it (C10). pdf.js itself is faked
// faithfully where it matters: a canvas takes one render() at a time,
// and a cancelled render rejects with RenderingCancelledException.

import { createApp, defineComponent, h, nextTick, type App } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  active: false,
  drawn: [] as number[],
  concurrentErrors: 0,
  stored: [] as { device: string; cfi: string | null }[],
  setPosition: undefined as unknown as ReturnType<typeof import("vitest").vi.fn>,
}));

function makePage(n: number) {
  return {
    getViewport: ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale, scale, rawDims: { pageWidth: 600, pageHeight: 800, pageX: 0, pageY: 0 }, rotation: 0 }),
    render: ({ canvas }: { canvas: HTMLCanvasElement }) => {
      let cancelled = false;
      let reject: (e: unknown) => void = () => {};
      const failure = state.active;
      if (!failure) state.active = true;
      const promise = new Promise<void>((resolve, rej) => {
        reject = rej;
        setTimeout(() => {
          if (failure) {
            state.concurrentErrors += 1;
            rej(new Error("Cannot use the same canvas during multiple render() operations."));
            return;
          }
          state.active = false;
          if (cancelled) rej(Object.assign(new Error("cancelled"), { name: "RenderingCancelledException" }));
          else {
            state.drawn.push(n);
            canvas.dataset.page = String(n);
            resolve();
          }
        }, 4);
      });
      void reject;
      return {
        promise,
        cancel: () => {
          cancelled = true;
        },
      };
    },
    getTextContent: async () => ({ items: [{ str: n === 9 ? "needle here" : "plain text", hasEOL: true }], styles: {} }),
    streamTextContent: () => ({}),
    cleanup: () => {},
  };
}

const fakeDoc = {
  numPages: 30,
  getPage: async (n: number) => makePage(n),
  getOutline: async () => [{ title: "Chapter Two", dest: "ch2", items: [] }],
  getDestination: async (name: string) => (name === "ch2" ? [{ num: 12 }] : null),
  getPageIndex: async (ref: { num: number }) => ref.num - 1,
  destroy: vi.fn(async () => {}),
};

vi.mock("../reader/pdf", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../reader/pdf")>()),
  loadPdfJs: vi.fn(async () => ({
    getDocument: () => ({ promise: Promise.resolve(fakeDoc) }),
    TextLayer: class {
      textDivs: HTMLElement[] = [];
      constructor(opts: { container: HTMLElement }) {
        const span = document.createElement("span");
        span.textContent = "x";
        opts.container.append(span);
        this.textDivs = [span];
      }
      async render() {}
      cancel() {}
    },
  })),
}));

const api = vi.hoisted(() => ({
  getLastReadPositions: vi.fn(),
  setLastReadPosition: vi.fn(async () => undefined),
  getAnnotations: vi.fn(async () => ({ bookmark: [] })),
  addBookmark: vi.fn(),
  removeAnnotation: vi.fn(async () => undefined),
}));
vi.mock("../reader/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../reader/api")>()), ...api }));
vi.mock("../library/api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/api")>()), fetchBook: vi.fn(async () => ({ title: "Test Book" })) }));

import PdfReader from "./PdfReader.vue";
import { deviceId } from "../reader/position";

let app: App | null = null;
let host: HTMLElement;

async function flush(rounds = 25) {
  for (let i = 0; i < rounds; i += 1) {
    await new Promise((r) => setTimeout(r, 2));
    await nextTick();
  }
}

async function mountReader() {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(defineComponent({ render: () => h(PdfReader, { bookId: "7" }) }));
  app.component("router-link", defineComponent({ render() { return h("a", this.$slots.default?.()); } }));
  app.mount(host);
  await flush();
}

const canvasEl = () => host.querySelector("canvas") as HTMLCanvasElement;
const buttonByText = (text: string) => Array.from(host.querySelectorAll("button")).find((b) => b.textContent?.trim().startsWith(text)) as HTMLButtonElement;
const key = (k: string, init: KeyboardEventInit = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

beforeEach(() => {
  state.active = false;
  state.drawn = [];
  state.concurrentErrors = 0;
  localStorage.clear();
  api.getLastReadPositions.mockResolvedValue([]);
  HTMLCanvasElement.prototype.getContext = (() => ({})) as never;
});

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
});

describe("PdfReader render race (C1)", () => {
  it("reopening at a saved page draws it once and shows no error", async () => {
    api.getLastReadPositions.mockResolvedValue([{ device: deviceId(), cfi: "17", pos_frac: 0.5 }]);
    await mountReader();

    expect(state.drawn).toEqual([17]);
    expect(state.concurrentErrors).toBe(0);
    expect(host.querySelector(".error")).toBeNull();
    expect(canvasEl().dataset.page).toBe("17");
  });

  it("five quick Next clicks end on the fifth page, never blank, never an error", async () => {
    api.getLastReadPositions.mockResolvedValue([{ device: deviceId(), cfi: "17", pos_frac: 0.5 }]);
    await mountReader();

    for (let i = 0; i < 5; i += 1) {
      buttonByText("Next").click();
      await new Promise((r) => setTimeout(r, 1));
    }
    await flush();

    expect(state.concurrentErrors).toBe(0);
    expect(host.querySelector(".error")).toBeNull();
    expect(canvasEl().dataset.page).toBe("22");
    expect(state.drawn[state.drawn.length - 1]).toBe(22);
    expect((host.querySelector('input[aria-label="Page number"]') as HTMLInputElement).value).toBe("22");
  });

  it("holding the arrow key behaves the same", async () => {
    await mountReader();
    for (let i = 0; i < 6; i += 1) {
      key("ArrowRight");
      await new Promise((r) => setTimeout(r, 1));
    }
    await flush();
    expect(state.concurrentErrors).toBe(0);
    expect(canvasEl().dataset.page).toBe("7");
  });
});

describe("PdfReader as a reader (C10)", () => {
  it("jumps with Home, End, PageDown and PageUp", async () => {
    await mountReader();
    key("End");
    await flush();
    expect(canvasEl().dataset.page).toBe("30");
    key("PageUp");
    await flush();
    expect(canvasEl().dataset.page).toBe("29");
    key("Home");
    await flush();
    expect(canvasEl().dataset.page).toBe("1");
  });

  it("lists the outline and goes to the entry's page", async () => {
    await mountReader();
    buttonByText("Contents").click();
    await flush(4);
    const entry = Array.from(host.querySelectorAll(".pdf-link")).find((b) => b.textContent === "Chapter Two") as HTMLButtonElement;
    expect(entry).toBeTruthy();
    entry.click();
    await flush();
    expect(canvasEl().dataset.page).toBe("12");
  });

  it("Ctrl+F opens a focused search box, and Enter finds the page with a hit", async () => {
    await mountReader();
    key("f", { ctrlKey: true });
    await flush(4);
    const box = host.querySelector('input[aria-label="Search in this PDF"]') as HTMLInputElement;
    expect(box).toBeTruthy();
    expect(document.activeElement).toBe(box);

    box.value = "needle";
    box.dispatchEvent(new Event("input"));
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await flush(60);
    expect(canvasEl().dataset.page).toBe("9");
    expect(host.querySelector(".pdf-find-status")?.textContent).toContain("1 of 1");
  });

  it("starts fit-to-width and remembers a different choice", async () => {
    await mountReader();
    expect(buttonByText("Fit width").classList.contains("active")).toBe(true);
    buttonByText("Fit page").click();
    await flush(4);
    expect(JSON.parse(localStorage.getItem("lexmechanic.pdf.zoom")!).mode).toBe("page");
  });

  it("bookmarks the page through the shared annotations API", async () => {
    // The title prompt is a dialog; answer it as the user would.
    const confirm = await import("./dialogs/confirm");
    vi.spyOn(confirm, "promptDialog").mockResolvedValue("Start of part two");
    api.addBookmark.mockResolvedValue({ type: "bookmark", title: "Start of part two", pos: "1", pos_type: "lexmechanic-simple-pos", timestamp: "" });
    await mountReader();
    buttonByText("Bookmark this page").click();
    await flush(6);
    expect(api.addBookmark).toHaveBeenCalledWith("7", "pdf", "Start of part two", "1");
    expect(buttonByText("Bookmarks").textContent).toContain("(1)");
  });
});
