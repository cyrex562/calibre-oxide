import { describe, expect, it } from "vitest";
import { parseSnippetSegments } from "./snippets";

const HL_START = String.fromCharCode(0x1c);
const HL_END = String.fromCharCode(0x1e);

describe("parseSnippetSegments", () => {
  it("returns the whole text as one unhighlighted segment with no markers", () => {
    expect(parseSnippetSegments("plain text")).toEqual([{ text: "plain text", highlighted: false }]);
  });

  it("splits a single highlighted run out of surrounding plain text", () => {
    const text = `before ${HL_START}match${HL_END} after`;
    expect(parseSnippetSegments(text)).toEqual([
      { text: "before ", highlighted: false },
      { text: "match", highlighted: true },
      { text: " after", highlighted: false },
    ]);
  });

  it("handles multiple highlighted runs", () => {
    const text = `${HL_START}one${HL_END} and ${HL_START}two${HL_END}`;
    expect(parseSnippetSegments(text)).toEqual([
      { text: "one", highlighted: true },
      { text: " and ", highlighted: false },
      { text: "two", highlighted: true },
    ]);
  });

  it("treats an unterminated highlight marker as running to the end", () => {
    const text = `plain ${HL_START}rest is highlighted`;
    expect(parseSnippetSegments(text)).toEqual([
      { text: "plain ", highlighted: false },
      { text: "rest is highlighted", highlighted: true },
    ]);
  });
});
