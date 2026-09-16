import { describe, expect, it } from "vitest";
import { wrapHighlightRange } from "./highlightDom";

function buildDoc(bodyHtml: string): Document {
  const doc = document.implementation.createHTMLDocument("");
  doc.body.innerHTML = bodyHtml;
  return doc;
}

describe("wrapHighlightRange", () => {
  it("wraps a range confined to a single text node in one <mark>", () => {
    const doc = buildDoc("<p>Hello world</p>");
    const textNode = doc.body.firstChild!.firstChild as Text;
    const range = doc.createRange();
    range.setStart(textNode, 0);
    range.setEnd(textNode, 5);

    wrapHighlightRange(range, "cx-highlight", { uuid: "abc" });

    const marks = doc.querySelectorAll("mark.cx-highlight");
    expect(marks.length).toBe(1);
    expect(marks[0].textContent).toBe("Hello");
    expect((marks[0] as HTMLElement).dataset.uuid).toBe("abc");
    expect(doc.body.textContent).toBe("Hello world");
  });

  it("wraps a range spanning multiple elements in several <mark>s, preserving the full text", () => {
    const doc = buildDoc("<p>Hello <b>brave</b> new world</p>");
    const p = doc.body.firstChild!;
    const helloText = p.firstChild as Text; // "Hello "
    const newWorldText = p.childNodes[2] as Text; // " new world"

    const range = doc.createRange();
    range.setStart(helloText, 2); // "llo "
    range.setEnd(newWorldText, 4); // " new"

    const originalText = range.toString();
    expect(originalText).toBe("llo brave new");

    wrapHighlightRange(range, "cx-highlight");

    const marks = doc.querySelectorAll("mark.cx-highlight");
    expect(marks.length).toBe(3);
    const reconstructed = Array.from(marks).map((m) => m.textContent).join("");
    expect(reconstructed).toBe(originalText);
    // Content outside the range survives untouched.
    expect(doc.body.textContent).toBe("Hello brave new world");
  });

  it("does nothing for a collapsed (empty) range", () => {
    const doc = buildDoc("<p>Hello</p>");
    const textNode = doc.body.firstChild!.firstChild as Text;
    const range = doc.createRange();
    range.setStart(textNode, 2);
    range.setEnd(textNode, 2);

    wrapHighlightRange(range, "cx-highlight");

    expect(doc.querySelectorAll("mark").length).toBe(0);
  });
});
