import { describe, expect, it } from "vitest";

import { parseStorageInfo, storageBanner } from "./storageBanner";

const local = { tier: "local_internal", journal_mode: "wal", in_use: [], write_blocked_reason: null };

describe("parseStorageInfo", () => {
  it("returns null for an older server that does not report storage", () => {
    expect(parseStorageInfo(undefined)).toBeNull();
    expect(parseStorageInfo({})).toBeNull();
  });

  it("reads the server's snake_case fields", () => {
    const info = parseStorageInfo({
      tier: "network",
      journal_mode: "delete",
      in_use: [{ signal: "calibre_running", blocking: false, message: "calibre is running" }],
      write_blocked_reason: null,
    });
    expect(info?.journalMode).toBe("delete");
    expect(info?.inUse[0]).toEqual({ signal: "calibre_running", blocking: false, message: "calibre is running" });
  });
});

describe("storageBanner", () => {
  it("says nothing for an ordinary local library", () => {
    expect(storageBanner(parseStorageInfo(local))).toBeNull();
    expect(storageBanner(null)).toBeNull();
  });

  it("recommends a local copy for large edits on network storage", () => {
    const banner = storageBanner(parseStorageInfo({ ...local, tier: "network", journal_mode: "delete" }));
    expect(banner?.level).toBe("info");
    expect(banner?.title).toMatch(/network/i);
    expect(banner?.detail).toMatch(/local copy/i);
    // Advice only: no external link in the text.
    expect(banner?.detail).not.toMatch(/https?:/);
  });

  it("warns, and names why, when another program has the library open", () => {
    const banner = storageBanner(
      parseStorageInfo({
        ...local,
        in_use: [{ signal: "exclusive_open_denied", blocking: true, message: "calibre has metadata.db open" }],
        write_blocked_reason: "calibre has metadata.db open",
      }),
    );
    expect(banner?.level).toBe("warning");
    expect(banner?.detail).toContain("calibre has metadata.db open");
  });

  it("puts the blocking reason ahead of the network advice", () => {
    const banner = storageBanner(
      parseStorageInfo({ ...local, tier: "network", write_blocked_reason: "someone else is editing" }),
    );
    expect(banner?.level).toBe("warning");
  });

  it("surfaces a soft in-use signal on a local library without blocking", () => {
    const banner = storageBanner(
      parseStorageInfo({ ...local, in_use: [{ signal: "calibre_running", blocking: false, message: "calibre is running" }] }),
    );
    expect(banner?.level).toBe("warning");
    expect(banner?.title).toMatch(/another program/i);
  });
});
