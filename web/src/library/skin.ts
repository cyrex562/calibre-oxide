// The application skin: "classic" (a dark take on the Windows 2000 / XP look,
// the default) or "modern" (the flat look that follows the system's light or
// dark setting). The choice lives on this device, in localStorage, and is
// applied as `data-skin` on <html>, which is what style.css and
// skin-classic.css key off. main.ts applies it before the app mounts and
// index.html applies it before the first paint, so there is no flash.

import { getItemCompat } from "./legacyStorage";

export type Skin = "classic" | "modern";

export const SKIN_KEY = "lexmechanic.skin";
export const DEFAULT_SKIN: Skin = "classic";
export const SKINS: { value: Skin; label: string; hint: string }[] = [
  { value: "classic", label: "Classic", hint: "Beveled controls and navy title bars, in the style of Windows 2000 and XP, in dark colors." },
  { value: "modern", label: "Modern", hint: "Flat controls. Follows the system's light or dark setting." },
];

export function isSkin(value: unknown): value is Skin {
  return value === "classic" || value === "modern";
}

/** The saved skin, or the default when nothing valid is stored (or storage is unavailable). */
export function getSkin(): Skin {
  try {
    const raw = getItemCompat(localStorage, SKIN_KEY);
    return isSkin(raw) ? raw : DEFAULT_SKIN;
  } catch {
    return DEFAULT_SKIN;
  }
}

/** Puts `skin` on <html> so the stylesheet switches. Does not save it. */
export function applySkin(skin: Skin, root: HTMLElement = document.documentElement): void {
  root.setAttribute("data-skin", skin);
}

/** Saves and applies `skin`. A storage that refuses the write still applies it for this session. */
export function setSkin(skin: Skin): Skin {
  try {
    localStorage.setItem(SKIN_KEY, skin);
  } catch {
    /* storage unavailable: the choice lasts until the page reloads */
  }
  applySkin(skin);
  return skin;
}

/** Reads the saved skin and applies it. Called once at start-up. */
export function applySavedSkin(): Skin {
  const skin = getSkin();
  applySkin(skin);
  return skin;
}
