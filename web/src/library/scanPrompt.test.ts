import { describe, expect, it } from "vitest";

import { describeScanOutcome, dismissKey, scanPromptHint, scanPromptText, shouldPrompt, type UntrackedCount } from "./scanPrompt";

const count = (over: Partial<UntrackedCount> = {}): UntrackedCount => ({ untracked: 0, at_least: false, tracked: 0, conclusive: true, ...over });

describe("scanPromptText", () => {
  it("asks the question in the words the prompt is meant to use", () => {
    expect(scanPromptText(count({ untracked: 1234 }))).toBe("This folder has 1,234 untracked book files — scan now?");
  });

  it("says 'at least' when the count stopped at its cap", () => {
    expect(scanPromptText(count({ untracked: 10000, at_least: true }))).toBe("This folder has at least 10,000 untracked book files — scan now?");
  });

  it("is singular for one file", () => {
    expect(scanPromptText(count({ untracked: 1 }))).toBe("This folder has 1 untracked book file — scan now?");
  });

  it("asks nothing when there is nothing to scan", () => {
    expect(scanPromptText(count())).toBeNull();
  });
});

describe("scanPromptHint", () => {
  it("promises the files are not moved, and warns when it will be long or incomplete", () => {
    expect(scanPromptHint(count({ untracked: 3 }))).toBe("Scanning adds them to the library without moving or changing any file.");
    expect(scanPromptHint(count({ untracked: 50000 }))).toMatch(/network share.*stopped and resumed/);
    expect(scanPromptHint(count({ untracked: 3, conclusive: false }))).toMatch(/could not be read.*more/);
  });
});

describe("shouldPrompt", () => {
  it("prompts for untracked files unless the user said not now", () => {
    expect(shouldPrompt(count({ untracked: 5 }), false)).toBe(true);
    expect(shouldPrompt(count({ untracked: 5 }), true)).toBe(false);
  });

  it("does not prompt before the count is known, or when it is zero", () => {
    expect(shouldPrompt(null, false)).toBe(false);
    expect(shouldPrompt(count(), false)).toBe(false);
  });

  it("remembers 'not now' per library", () => {
    expect(dismissKey("/a")).not.toBe(dismissKey("/b"));
  });
});

describe("describeScanOutcome", () => {
  it("reports books added, failures, and an early stop", () => {
    expect(describeScanOutcome({ added: [1, 2, 3], conclusive: true, failed: [] })).toBe("3 books added");
    expect(describeScanOutcome({ added: [1], conclusive: true, failed: [{}] })).toBe("1 book added · 1 could not be read");
    expect(describeScanOutcome({ added: [], conclusive: false, cancelled: true, failed: [] })).toBe("0 books added · stopped early — scan again to carry on");
    expect(describeScanOutcome({ added: [], conclusive: false, failed: [] })).toBe("0 books added · some folders could not be read");
  });
});
