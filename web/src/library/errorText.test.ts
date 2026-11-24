import { describe, expect, it } from "vitest";

import { humanError, isUnreachable, UNREACHABLE } from "./errorText";

describe("humanError", () => {
  it("says the server is unreachable for the browser's own network failures", () => {
    expect(humanError(new TypeError("Failed to fetch"))).toBe(UNREACHABLE);
    expect(humanError(new TypeError("Load failed"))).toBe(UNREACHABLE);
    expect(humanError(new Error("NetworkError when attempting to fetch resource."))).toBe(UNREACHABLE);
    expect(isUnreachable(new TypeError("Failed to fetch"))).toBe(true);
    expect(isUnreachable(new Error("GET /x failed: 500 Internal Server Error"))).toBe(false);
  });

  it("keeps the server's own explanation and drops the status line and the command-line flag", () => {
    const e = new Error("GET /plugins/catalog failed: 503 Service Unavailable — plugins are not enabled on this server -- start it with --plugin-dir <path>");
    const text = humanError(e);
    expect(text).toContain("plugins are not enabled on this server");
    expect(text).not.toContain("--plugin-dir");
    expect(text).not.toContain("503");
    expect(text).not.toContain("/plugins/catalog");
  });

  it("maps a bare status to a sentence", () => {
    expect(humanError(new Error("GET /ajax/search failed: 404 Not Found"))).toMatch(/does not have that/);
    expect(humanError(new Error("POST /cdb/set-fields failed: 403 Forbidden"))).toMatch(/permission/);
    expect(humanError(new Error("GET /ajax/books failed: 500 Internal Server Error"))).toMatch(/error 500/);
    expect(humanError(new Error("GET /ajax/books failed: 504 Gateway Timeout"))).toMatch(/too long/);
  });

  it("takes the server's TTS hint out of a plain message", () => {
    const text = humanError("TTS is not configured on this server -- start it with --tts-voice <path-to-voice.onnx>");
    expect(text).toBe("TTS is not configured on this server");
  });

  it("passes an ordinary message through and never returns an empty string", () => {
    expect(humanError(new Error("That folder is inside the library already."))).toBe("That folder is inside the library already.");
    expect(humanError("")).toBe("Something went wrong.");
    expect(humanError(undefined)).toBe("Something went wrong.");
  });
});
