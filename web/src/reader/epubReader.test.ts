// EPUB reader pieces: Read aloud wording (C9), the selection bar's
// keyboard access, locating a search hit, and removing highlights.

import { describe, expect, it, vi } from "vitest";

import { findMatches } from "./search";
import { indexText, pickMatch, rangeForOffsets } from "./domText";
import { unwrapHighlight, wrapHighlightRange } from "./highlightDom";
import { createSelectionBar, FOCUS_BAR_KEY } from "./selectionBar";
import { isTtsNotSetUp, TTS_NOT_SET_UP, ttsErrorText } from "./tts";

describe("Read aloud errors (C9)", () => {
  const raw = "TTS is not configured on this server -- start it with --tts-voice <path-to-voice.onnx>";

  it("never shows a command-line flag for a missing voice", () => {
    expect(ttsErrorText(new Error(raw), 503)).toBe(TTS_NOT_SET_UP);
    expect(ttsErrorText(new Error(raw))).not.toContain("--");
  });

  it("understands a machine code from the server too", () => {
    expect(isTtsNotSetUp(503, "tts_not_configured")).toBe(true);
    expect(isTtsNotSetUp(null, "TTS_DISABLED")).toBe(true);
    expect(ttsErrorText(new Error("tts_unavailable"), 503)).toBe(TTS_NOT_SET_UP);
  });

  it("keeps other failures plain, and still free of flags", () => {
    expect(isTtsNotSetUp(500, "boom")).toBe(false);
    expect(ttsErrorText(new Error("Failed to fetch"))).toMatch(/can't reach/i);
    expect(ttsErrorText(new Error("synthesis failed, retry with --verbose"))).not.toContain("--");
  });
});

describe("selection bar keyboard access", () => {
  function setup() {
    const run = [vi.fn(), vi.fn(), vi.fn()];
    const escaped = vi.fn();
    const bar = createSelectionBar(document, "bar", "Selection actions", escaped);
    bar.setActions([
      { label: "Highlight", description: "Highlight the selected text", run: run[0] },
      { label: "Copy", run: run[1] },
      { label: "Look up", run: run[2] },
    ]);
    document.body.appendChild(bar.el);
    return { bar, run, escaped };
  }
  const press = (el: Element, key: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

  it("is a labelled toolbar with named buttons", () => {
    const { bar } = setup();
    expect(bar.el.getAttribute("role")).toBe("toolbar");
    expect(bar.el.getAttribute("aria-label")).toBe("Selection actions");
    expect(bar.el.querySelector("button")?.getAttribute("aria-label")).toBe("Highlight the selected text");
    bar.el.remove();
  });

  it("takes focus only while shown, then arrows move along it", () => {
    const { bar } = setup();
    expect(bar.focusFirst()).toBe(false);
    bar.showAt(10, 10);
    expect(bar.focusFirst()).toBe(true);
    const buttons = Array.from(bar.el.querySelectorAll("button"));
    expect(document.activeElement).toBe(buttons[0]);
    press(buttons[0], "ArrowRight");
    expect(document.activeElement).toBe(buttons[1]);
    press(buttons[1], "ArrowLeft");
    press(buttons[0], "ArrowLeft");
    expect(document.activeElement).toBe(buttons[2]);
    bar.el.remove();
  });

  it("closes on Escape and tells the reader to take focus back", () => {
    const { bar, escaped } = setup();
    bar.showAt(0, 0);
    bar.focusFirst();
    press(bar.el.querySelector("button")!, "Escape");
    expect(bar.isVisible()).toBe(false);
    expect(escaped).toHaveBeenCalled();
    bar.el.remove();
  });

  it("buttons are real buttons that act on Enter/click", () => {
    const { bar, run } = setup();
    bar.showAt(0, 0);
    bar.el.querySelectorAll("button")[1].click();
    expect(run[1]).toHaveBeenCalled();
    expect(FOCUS_BAR_KEY).toBe("h");
    bar.el.remove();
  });
});

describe("locating a search hit in the chapter", () => {
  const html = "<p>The cat sat.</p><p>Another cat, and a third <em>cat</em> too.</p><script>cat()</script>";

  function chapter() {
    const doc = document.implementation.createHTMLDocument("c");
    doc.body.innerHTML = html;
    return doc;
  }

  it("indexes visible text only", () => {
    const index = indexText(chapter().body);
    expect(index.text).not.toContain("cat()");
    expect(index.text).toContain("The cat sat.");
  });

  it("finds the n-th hit's exact range, across element boundaries", () => {
    const doc = chapter();
    const index = indexText(doc.body);
    const found = findMatches(index.text, "cat", {});
    expect(found).toHaveLength(3);
    // The listed (search-panel) matches are the same three.
    const listed = findMatches("The cat sat. Another cat, and a third cat too.", "cat", {});
    const hit = pickMatch(found, 2, listed[2], listed.length)!;
    const range = rangeForOffsets(doc, index, hit.index, hit.index + hit.text.length)!;
    expect(range.toString()).toBe("cat");
    // The third hit is the one inside <em>, not the first in the chapter.
    expect(range.startContainer.parentElement?.tagName).toBe("EM");
  });

  it("gives up honestly when the two texts disagree and nothing matches by context", () => {
    const doc = chapter();
    const found = findMatches(indexText(doc.body).text, "cat", {});
    const listed = { index: 0, text: "cat", context: "something else entirely", contextOffset: 0 };
    expect(pickMatch(found, 0, listed, found.length + 1)).toBeNull();
  });
});

describe("removing a highlight", () => {
  it("unwraps every fragment and restores the text", () => {
    const doc = document.implementation.createHTMLDocument("c");
    doc.body.innerHTML = "<p>alpha <b>beta</b> gamma</p>";
    const range = doc.createRange();
    range.setStart(doc.body.querySelector("p")!.firstChild!, 2);
    range.setEnd(doc.body.querySelector("p")!.lastChild!, 3);
    wrapHighlightRange(range, "cx-highlight", { uuid: "u1" });
    expect(doc.querySelectorAll("mark[data-uuid='u1']").length).toBeGreaterThan(1);

    const removed = unwrapHighlight(doc, "u1");
    expect(removed).toBeGreaterThan(1);
    expect(doc.querySelectorAll("mark")).toHaveLength(0);
    expect(doc.body.innerHTML).toBe("<p>alpha <b>beta</b> gamma</p>");
  });

  it("leaves other highlights alone", () => {
    const doc = document.implementation.createHTMLDocument("c");
    doc.body.innerHTML = '<p><mark class="cx-highlight" data-uuid="a">x</mark> <mark class="cx-highlight" data-uuid="b">y</mark></p>';
    expect(unwrapHighlight(doc, "a")).toBe(1);
    expect(doc.querySelectorAll("mark")).toHaveLength(1);
    expect(unwrapHighlight(doc, "missing")).toBe(0);
  });
});
