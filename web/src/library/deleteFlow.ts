// Deleting books, in one place: the confirm that names the titles, the call,
// the "Moved N books to Trash · Undo" toast, and the failures.
//
// Every delete in the UI (the toolbar and Del key, the details panel, the
// duplicate review's per-copy Delete) goes through `trashBooksFlow`, so the
// wording is the same everywhere: books go to the library Trash, not into
// the void, and can be put back.
//
// State is module-level and reactive; `DeleteConfirm.vue` and `UndoToast.vue`
// (mounted once, by the library view) render it. Keeping it here means a
// component deep in the tree can ask without prop drilling, and the logic is
// testable without mounting anything.

import { reactive } from "vue";

import { deleteBooks, describeDeleteFailures, restoreDeletedBooks, type DeleteOutcome } from "./api";

export interface DeleteItem {
  id: number;
  title: string;
}

/** How many titles the confirm names before saying "and N more". */
export const NAMED_TITLES = 5;

/** The one question every delete asks. */
export function trashQuestion(count: number): string {
  return `Move ${count.toLocaleString("en-US")} ${count === 1 ? "book" : "books"} to the library Trash?`;
}

/** The toast text after a delete. */
export function movedToTrashMessage(count: number): string {
  return `Moved ${count.toLocaleString("en-US")} ${count === 1 ? "book" : "books"} to Trash`;
}

export interface TrashConfirmText {
  question: string;
  /** Up to `NAMED_TITLES` titles. */
  titles: string[];
  /** Titles not named. */
  moreCount: number;
  /** Set when some of the books are not in the current search results. */
  hiddenNote: string | null;
  note: string;
}

/** What the confirm shows. `hiddenCount` is how many of `items` the current search hides. */
export function describeTrashConfirm(items: DeleteItem[], hiddenCount = 0): TrashConfirmText {
  const titles = items.slice(0, NAMED_TITLES).map((i) => i.title || "(untitled)");
  const hidden = Math.max(0, Math.min(hiddenCount, items.length));
  return {
    question: trashQuestion(items.length),
    titles,
    moreCount: Math.max(0, items.length - titles.length),
    hiddenNote: hidden > 0 ? `${hidden.toLocaleString("en-US")} of these ${hidden === 1 ? "is" : "are"} hidden by the current search.` : null,
    note: "They can be restored from the Trash for a while.",
  };
}

export interface ConfirmState {
  open: boolean;
  text: TrashConfirmText | null;
  /** Extra cautions ("this is the last copy"), shown above the buttons. */
  warnings: string[];
}

export const confirmState = reactive<ConfirmState>({ open: false, text: null, warnings: [] });
let pendingConfirm: ((ok: boolean) => void) | null = null;

/** Opens the confirm and resolves with the answer. A second ask refuses the first. */
export function confirmTrash(items: DeleteItem[], opts: { hiddenCount?: number; warnings?: string[] } = {}): Promise<boolean> {
  pendingConfirm?.(false);
  confirmState.text = describeTrashConfirm(items, opts.hiddenCount ?? 0);
  confirmState.warnings = opts.warnings ?? [];
  confirmState.open = true;
  return new Promise<boolean>((resolve) => {
    pendingConfirm = resolve;
  });
}

export function answerTrashConfirm(ok: boolean): void {
  confirmState.open = false;
  const resolve = pendingConfirm;
  pendingConfirm = null;
  resolve?.(ok);
}

export interface ToastState {
  message: string;
  actionLabel: string | null;
  /** Shown instead of the action once it has run. */
  result: string | null;
  busy: boolean;
  onAction: (() => Promise<string | void>) | null;
}

export const toastState = reactive<{ toast: ToastState | null }>({ toast: null });
let toastTimer: ReturnType<typeof setTimeout> | null = null;

export function dismissToast(): void {
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = null;
  toastState.toast = null;
}

/** A message with one optional action (Undo). Goes away by itself after `ms`. */
export function showToast(message: string, action?: { label: string; run: () => Promise<string | void> }, ms = 12000): void {
  dismissToast();
  toastState.toast = { message, actionLabel: action?.label ?? null, result: null, busy: false, onAction: action?.run ?? null };
  toastTimer = setTimeout(dismissToast, ms);
}

/** Runs the toast's action once; the result replaces the message. */
export async function runToastAction(): Promise<void> {
  const t = toastState.toast;
  if (!t || !t.onAction || t.busy) return;
  if (toastTimer) clearTimeout(toastTimer);
  t.busy = true;
  try {
    t.result = (await t.onAction()) ?? "Done";
  } catch (e) {
    t.result = e instanceof Error ? e.message : String(e);
  }
  t.busy = false;
  t.actionLabel = null;
  toastTimer = setTimeout(dismissToast, 5000);
}

export interface TrashFlowOptions {
  /** How many of the books the current search hides. */
  hiddenCount?: number;
  warnings?: string[];
  /** Runs after the books were put back by Undo, so the caller can refresh. */
  onRestored?: () => void;
}

export interface TrashFlowResult {
  cancelled: boolean;
  outcome: DeleteOutcome | null;
  /** The server's own words for what could not be deleted; "" when all went. */
  failureText: string;
}

/**
 * Confirms, deletes, and offers Undo. A rejected request propagates, so the
 * caller shows it where it shows its other errors; a partial failure comes
 * back in the result and is also reflected in the toast.
 */
export async function trashBooksFlow(items: DeleteItem[], opts: TrashFlowOptions = {}): Promise<TrashFlowResult> {
  if (items.length === 0) return { cancelled: true, outcome: null, failureText: "" };
  if (!(await confirmTrash(items, opts))) return { cancelled: true, outcome: null, failureText: "" };
  const outcome = await deleteBooks(items.map((i) => i.id));
  const failureText = describeDeleteFailures(outcome);
  if (outcome.deleted.length > 0) {
    const restoredIds = [...outcome.deleted];
    showToast(movedToTrashMessage(restoredIds.length), {
      label: "Undo",
      run: async () => {
        const n = await restoreDeletedBooks(restoredIds);
        opts.onRestored?.();
        return n === restoredIds.length ? `Restored ${n.toLocaleString("en-US")} ${n === 1 ? "book" : "books"}` : `Restored ${n} of ${restoredIds.length} (the rest are in the Trash)`;
      },
    });
  }
  return { cancelled: false, outcome, failureText };
}
