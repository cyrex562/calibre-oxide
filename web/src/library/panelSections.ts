// Which sections of the book details panel are open.
//
// Several can be open at once. The state lives in localStorage, read
// and written inside try/catch: storage can be missing, full or
// blocked (private windows, previews), and the panel must still render.

import { getItemCompat } from "./legacyStorage";

export const SECTION_IDS = ["cover", "preview", "metadata", "files", "actions"] as const;
export type SectionId = (typeof SECTION_IDS)[number];

export const SECTIONS_KEY = "lexmechanic.details-sections";

export type SectionState = Record<SectionId, boolean>;

/** Preview starts closed: opening it fetches and renders the PDF. */
export const DEFAULT_SECTIONS: SectionState = { cover: true, preview: false, metadata: true, files: true, actions: true };

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** Reads stored open/closed state, field by field, falling back to defaults. */
export function loadSections(storage: StorageLike | null = defaultStorage()): SectionState {
  const out = { ...DEFAULT_SECTIONS };
  try {
    const raw = storage ? getItemCompat(storage, SECTIONS_KEY) : null;
    if (!raw) return out;
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    if (typeof parsed !== "object" || parsed === null) return out;
    for (const id of SECTION_IDS) if (typeof parsed[id] === "boolean") out[id] = parsed[id] as boolean;
  } catch {
    // corrupt or unavailable: defaults
  }
  return out;
}

export function saveSections(state: SectionState, storage: StorageLike | null = defaultStorage()): void {
  try {
    storage?.setItem(SECTIONS_KEY, JSON.stringify(state));
  } catch {
    // not persisted; the panel still works
  }
}

export function toggleSection(state: SectionState, id: SectionId): SectionState {
  return { ...state, [id]: !state[id] };
}
