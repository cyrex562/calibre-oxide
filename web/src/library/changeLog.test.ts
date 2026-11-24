import { describe, expect, it } from "vitest";

import { changeLogBanner, parseChangeLogStatus } from "./changeLog";

describe("parseChangeLogStatus", () => {
  it("reads the server's status", () => {
    expect(parseChangeLogStatus({ behind: true, missed: 3, first_at: 1700000000, last_error: "disk full" })).toEqual({ behind: true, missed: 3, firstAt: 1700000000, lastError: "disk full" });
    expect(parseChangeLogStatus({ behind: false, missed: 0, first_at: null, last_error: null })).toEqual({ behind: false, missed: 0, firstAt: null, lastError: "" });
  });

  it("is null for anything that is not a status", () => {
    for (const v of [null, undefined, "x", 3, {}, { missed: 2 }, { behind: "yes" }]) expect(parseChangeLogStatus(v)).toBeNull();
  });
});

describe("changeLogBanner", () => {
  it("is absent when the log is in step or nothing is known", () => {
    expect(changeLogBanner(null)).toBeNull();
    expect(changeLogBanner({ behind: false, missed: 0, firstAt: null, lastError: "" })).toBeNull();
  });

  it("says how many changes were missed and why", () => {
    expect(changeLogBanner({ behind: true, missed: 12, firstAt: 1, lastError: "disk full" })?.text).toBe("The library's change log missed 12 changes (last error: disk full).");
    expect(changeLogBanner({ behind: true, missed: 1, firstAt: 1, lastError: "" })?.text).toBe("The library's change log missed 1 change.");
    expect(changeLogBanner({ behind: true, missed: 1500, firstAt: 1, lastError: "x" })?.text).toContain("1,500 changes");
  });
});
