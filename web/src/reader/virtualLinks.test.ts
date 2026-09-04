import { describe, expect, it } from "vitest";
import { anchorLinkData, decodeVirtualComponent, rewriteVirtualLinks } from "./virtualLinks";

describe("decodeVirtualComponent", () => {
  it("decodes a base64 name with no fragment", () => {
    const encoded = btoa("chap1.xhtml");
    expect(decodeVirtualComponent(encoded)).toEqual({ name: "chap1.xhtml", frag: "" });
  });

  it("decodes a base64 name with a fragment", () => {
    const encoded = `${btoa("chap2.xhtml")}#target`;
    expect(decodeVirtualComponent(encoded)).toEqual({ name: "chap2.xhtml", frag: "target" });
  });

  it("returns null for invalid base64", () => {
    expect(decodeVirtualComponent("not valid base64!!!")).toBeNull();
  });
});

describe("rewriteVirtualLinks", () => {
  it("replaces every link_uid|...| occurrence via the resolver", () => {
    const linkUid = "abc123";
    const name = btoa("style.css");
    const text = `url(${linkUid}|${name}|)`;
    const out = rewriteVirtualLinks(text, linkUid, (link) => (link.name === "style.css" ? "blob://xyz" : null));
    expect(out).toBe("url(blob://xyz)");
  });

  it("leaves the original text alone when the resolver declines", () => {
    const linkUid = "abc123";
    const name = btoa("style.css");
    const text = `url(${linkUid}|${name}|)`;
    const out = rewriteVirtualLinks(text, linkUid, () => null);
    expect(out).toBe(text);
  });

  it("handles a link_uid containing regex-special characters", () => {
    const linkUid = "a.b+c";
    const name = btoa("x.css");
    const text = `${linkUid}|${name}|`;
    const out = rewriteVirtualLinks(text, linkUid, () => "resolved");
    expect(out).toBe("resolved");
  });
});

describe("anchorLinkData", () => {
  it("parses a real data-{link_uid} JSON attribute", () => {
    const el = document.createElement("a");
    el.setAttribute("data-abc123", JSON.stringify({ name: "chap2.xhtml", frag: "target" }));
    expect(anchorLinkData(el, "abc123")).toEqual({ name: "chap2.xhtml", frag: "target" });
  });

  it("returns null when the attribute is absent", () => {
    const el = document.createElement("a");
    expect(anchorLinkData(el, "abc123")).toBeNull();
  });

  it("marks a missing-resource link", () => {
    const el = document.createElement("a");
    el.setAttribute("data-abc123", JSON.stringify({ name: "gone.xhtml", frag: "", missing: true }));
    expect(anchorLinkData(el, "abc123")?.missing).toBe(true);
  });
});
