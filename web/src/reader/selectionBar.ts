// The small action bar the EPUB reader shows over a selection (and over
// a highlight, to remove it).
//
// It lives inside the book's own document, so it sits in the same
// coordinate space as the text it points at. It used to be a row of
// bare buttons shown only on mouse-up, which left keyboard users with
// no way to reach "Highlight" or "Copy". Now it is a labelled toolbar:
// it appears after a keyboard selection too, Alt+H moves focus into it,
// the arrow keys move along it, and Escape closes it and hands focus
// back to the text.

export interface BarAction {
  label: string;
  /** Spoken name when it differs from the label. */
  description?: string;
  run: () => void;
}

export interface SelectionBar {
  el: HTMLElement;
  /** Replaces the buttons. */
  setActions: (actions: BarAction[]) => void;
  /** Shows the bar with its top-left at document coordinates. */
  showAt: (left: number, top: number) => void;
  hide: () => void;
  isVisible: () => boolean;
  /** Moves focus to the first button. */
  focusFirst: () => boolean;
}

/** The key that moves focus into a visible bar. */
export const FOCUS_BAR_KEY = "h";

const BAR_CSS = "position:absolute;z-index:1000;display:none;gap:1px;border-radius:4px;overflow:hidden;box-shadow:0 2px 8px rgba(0,0,0,0.3);font:14px sans-serif;";
const BUTTON_CSS = "padding:0.3em 0.7em;border:none;background:#2a6df4;color:#fff;cursor:pointer;font:inherit;";

export function createSelectionBar(doc: Document, id: string, label: string, onEscape: () => void = () => {}): SelectionBar {
  const el = doc.createElement("div");
  el.id = id;
  el.setAttribute("role", "toolbar");
  el.setAttribute("aria-label", label);
  el.title = `${label} (Alt+${FOCUS_BAR_KEY.toUpperCase()} to move here)`;
  el.style.cssText = BAR_CSS;

  const buttons = () => Array.from(el.querySelectorAll<HTMLButtonElement>("button"));

  el.addEventListener("keydown", (event) => {
    const list = buttons();
    const at = list.indexOf(doc.activeElement as HTMLButtonElement);
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      list[(at + 1) % list.length]?.focus();
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      list[(at - 1 + list.length) % list.length]?.focus();
    } else if (event.key === "Escape") {
      hide();
      onEscape();
    } else {
      return;
    }
    event.preventDefault();
  });

  function setActions(actions: BarAction[]) {
    el.textContent = "";
    for (const action of actions) {
      const button = doc.createElement("button");
      button.type = "button";
      button.textContent = action.label;
      if (action.description) button.setAttribute("aria-label", action.description);
      button.style.cssText = BUTTON_CSS;
      // Without this, the mousedown on the button itself would collapse
      // the very selection it is meant to act on before the click
      // handler ever runs.
      button.addEventListener("mousedown", (e) => e.preventDefault());
      button.addEventListener("click", action.run);
      el.appendChild(button);
    }
  }

  function hide() {
    el.style.display = "none";
  }

  return {
    el,
    setActions,
    showAt(left, top) {
      el.style.left = `${Math.max(0, left)}px`;
      el.style.top = `${Math.max(0, top)}px`;
      el.style.display = "flex";
    },
    hide,
    isVisible: () => el.style.display !== "none",
    focusFirst() {
      const first = buttons()[0];
      if (!first || el.style.display === "none") return false;
      first.focus();
      return true;
    },
  };
}
