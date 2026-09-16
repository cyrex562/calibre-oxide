import { describe, expect, it } from "vitest";
import { decodeBoundary, encodeBoundary, rangeFromEncoded } from "./highlightRange";

function buildDoc(bodyHtml: string): Document {
  const doc = document.implementation.createHTMLDocument("");
  doc.body.innerHTML = bodyHtml;
  return doc;
}

describe("encodeBoundary / decodeBoundary", () => {
  it("round-trips a real text-node position", () => {
    const doc = buildDoc("<p>Hello <b>world</b></p>");
    const textNode = doc.body.firstChild!.firstChild!; // the "Hello " text node
    const encoded = encodeBoundary(doc.body, 3, textNode, 2);
    expect(encoded).not.toBeNull();
    const decoded = decodeBoundary(encoded!);
    expect(decoded).toEqual({ spineIndex: 3, path: "0.0", offset: 2 });
  });

  it("returns null for a node that isn't a descendant of root", () => {
    const doc = buildDoc("<p>Hello</p>");
    const other = buildDoc("<p>Other</p>");
    expect(encodeBoundary(doc.body, 0, other.body.firstChild!, 0)).toBeNull();
  });

  it("returns null for a malformed string", () => {
    expect(decodeBoundary("not-a-real-encoding")).toBeNull();
    expect(decodeBoundary("calibre-oxide-simple-range:abc:0.0:1")).toBeNull();
  });
});

describe("rangeFromEncoded", () => {
  it("reconstructs a real range spanning the encoded boundaries", () => {
    const doc = buildDoc("<p>Hello world</p>");
    const textNode = doc.body.firstChild!.firstChild!;
    const start = encodeBoundary(doc.body, 5, textNode, 0)!;
    const end = encodeBoundary(doc.body, 5, textNode, 5)!;
    const range = rangeFromEncoded(doc, 5, start, end);
    expect(range?.toString()).toBe("Hello");
  });

  it("returns null when the boundary's spine index doesn't match", () => {
    const doc = buildDoc("<p>Hello world</p>");
    const textNode = doc.body.firstChild!.firstChild!;
    const start = encodeBoundary(doc.body, 5, textNode, 0)!;
    const end = encodeBoundary(doc.body, 5, textNode, 5)!;
    expect(rangeFromEncoded(doc, 6, start, end)).toBeNull();
  });

  it("returns null when the path no longer resolves against this document", () => {
    const doc = buildDoc("<p>Hello world</p>");
    const start = "calibre-oxide-simple-range:0:0.0:0";
    const end = "calibre-oxide-simple-range:0:9.9:1"; // no such path in this doc
    expect(rangeFromEncoded(doc, 0, start, end)).toBeNull();
  });
});
