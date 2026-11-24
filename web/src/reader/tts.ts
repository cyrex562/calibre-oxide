// What a failed "Read aloud" says to a person.
//
// The server's answer when no voice is configured is written for
// whoever starts it ("TTS is not configured on this server -- start it
// with --tts-voice <path-to-voice.onnx>"). That is not something a
// reader can act on, so it never reaches the screen: any "not set up"
// answer -- by its text, a status 503, or a machine code -- becomes one
// plain sentence.

import { humanError } from "../library/errorText";

export const TTS_NOT_SET_UP = "Read aloud isn't set up on this server.";

const NOT_SET_UP = [/tts[\s_-]*(is[\s_-]*)?(not[\s_-]*configured|disabled|unavailable|not[\s_-]*enabled)/i, /--tts-/i, /\.onnx/i, /not configured on this server/i];

/** Whether a failed `/tts/synthesize` means "this server has no voice". */
export function isTtsNotSetUp(status: number | null, body: string): boolean {
  if (NOT_SET_UP.some((re) => re.test(body))) return true;
  return status === 503 && /tts|voice|read aloud/i.test(body);
}

/** One plain sentence for a Read aloud failure. */
export function ttsErrorText(error: unknown, status: number | null = null): string {
  const raw = error instanceof Error ? error.message : String(error ?? "");
  if (isTtsNotSetUp(status, raw)) return TTS_NOT_SET_UP;
  const plain = humanError(error);
  // Whatever else it was, never show a command-line flag.
  return /--[a-z]/i.test(plain) ? "Read aloud failed. Try again." : plain;
}
