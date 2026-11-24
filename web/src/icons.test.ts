import { createApp, h } from "vue";
import { describe, expect, it } from "vitest";
import AppIcon from "./components/AppIcon.vue";
import { ICONS, iconComponent } from "./icons";
import { LIBRARY_ACTIONS, TOOLBAR_LAYOUT } from "./library/actions";

// Icon names that components pass by hand rather than via an action.
const HAND_PICKED = [
  "vl", "sort", "folder_saved_search", "fts", "search", "h-ellipsis",
  "user_profile", "tags", "series", "publisher", "languages", "rating", "chevron-down",
];

describe("icon mapping", () => {
  it("resolves every icon an action declares to a Lucide component", () => {
    for (const a of LIBRARY_ACTIONS) {
      if (a.icon === undefined) continue;
      expect(iconComponent(a.icon), `action ${a.id} uses unmapped icon "${a.icon}"`).toBeTruthy();
    }
  });

  it("resolves every toolbar menu icon", () => {
    for (const item of TOOLBAR_LAYOUT) {
      if (item.kind === "menu") {
        expect(iconComponent(item.icon), `menu ${item.id} icon "${item.icon}"`).toBeTruthy();
      }
    }
  });

  it("resolves the icons components pick by hand", () => {
    for (const name of HAND_PICKED) expect(ICONS[name], name).toBeTruthy();
  });

  it("renders an inline SVG in currentColor, not an image", () => {
    const host = document.createElement("div");
    createApp({ render: () => h(AppIcon, { name: "add_book" }) }).mount(host);
    expect(host.querySelector("svg")).not.toBeNull();
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")!.getAttribute("stroke")).toBe("currentColor");
  });

  it("renders nothing for an unknown name", () => {
    const host = document.createElement("div");
    createApp({ render: () => h(AppIcon, { name: "nope" }) }).mount(host);
    expect(host.querySelector("svg")).toBeNull();
  });
});
