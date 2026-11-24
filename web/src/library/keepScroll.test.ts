import { afterEach, describe, expect, it } from "vitest";
import { createApp, defineComponent, h, KeepAlive, nextTick, ref, type App } from "vue";

import { useKeepScroll } from "./keepScroll";

let app: App | null = null;
let host: HTMLElement | null = null;

afterEach(() => {
  app?.unmount();
  host?.remove();
  app = null;
  host = null;
});

const Scrolly = defineComponent({
  setup() {
    const el = ref<HTMLElement | null>(null);
    useKeepScroll(el);
    return () => h("div", { ref: el, class: "scrolly" });
  },
});

async function settle() {
  for (let i = 0; i < 4; i++) await nextTick();
}

describe("useKeepScroll", () => {
  it("puts the scroll offset back when a parked view is shown again", async () => {
    host = document.createElement("div");
    document.body.appendChild(host);
    const away = ref(false);
    app = createApp({ render: () => h(KeepAlive, null, { default: () => (away.value ? h("p") : h(Scrolly)) }) });
    app.mount(host);
    await settle();

    const el = host.querySelector<HTMLElement>(".scrolly")!;
    el.scrollTop = 240;
    el.dispatchEvent(new Event("scroll"));

    away.value = true;
    await settle();
    // A detached element reads 0 (and may report it): the offset must survive that.
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));

    away.value = false;
    await settle();
    expect(host.querySelector<HTMLElement>(".scrolly")!.scrollTop).toBe(240);
  });
});
