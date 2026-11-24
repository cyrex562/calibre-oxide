import { describe, expect, it } from "vitest";
import { getItemCompat, legacyKey } from "./legacyStorage";

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k: string) => (data.has(k) ? (data.get(k) as string) : null),
    setItem: (k: string, v: string) => void data.set(k, v),
    removeItem: (k: string) => void data.delete(k),
  };
}

describe("legacyKey", () => {
  it("maps the new prefixes back to the old one", () => {
    expect(legacyKey("lexmechanic.layout")).toBe("calibre-oxide.layout");
    expect(legacyKey("lexmechanic-device-id")).toBe("calibre-oxide-device-id");
  });
  it("leaves keys that are not ours alone", () => {
    expect(legacyKey("theme")).toBeNull();
    expect(legacyKey("lexmechanicalish")).toBeNull();
  });
});

describe("getItemCompat", () => {
  it("reads the new key", () => {
    const s = memoryStorage({ "lexmechanic.layout": "new", "calibre-oxide.layout": "old" });
    expect(getItemCompat(s, "lexmechanic.layout")).toBe("new");
  });

  it("falls back to the old key, writes the new one and drops the old one", () => {
    const s = memoryStorage({ "calibre-oxide.layout": "old" });
    expect(getItemCompat(s, "lexmechanic.layout")).toBe("old");
    expect(s.data.get("lexmechanic.layout")).toBe("old");
    expect(s.data.has("calibre-oxide.layout")).toBe(false);
    expect(getItemCompat(s, "lexmechanic.layout")).toBe("old");
  });

  it("returns null when neither exists", () => {
    expect(getItemCompat(memoryStorage(), "lexmechanic.layout")).toBeNull();
  });

  it("still returns the value when the new key cannot be written", () => {
    const s = memoryStorage({ "calibre-oxide-device-id": "abc" });
    s.setItem = () => {
      throw new Error("quota");
    };
    expect(getItemCompat(s, "lexmechanic-device-id")).toBe("abc");
  });
});
