// Promise-returning replacements for window.confirm / prompt / alert, built
// on Dialog: they look like the rest of the app, hold focus, and work in the
// desktop shell, where the browser's own boxes are plain and blocking.
//
// Each call mounts a tiny app on <body> and unmounts it when answered, so
// callers need no template for it.

import { createApp, h, type Component } from "vue";

import ConfirmDialog from "./ConfirmDialog.vue";

export interface ConfirmOptions {
  title?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Style the confirm button as destructive and put focus on Cancel. */
  danger?: boolean;
}

function ask<T>(props: Record<string, unknown>, fallback: T): Promise<T> {
  return new Promise<T>((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const app = createApp({
      render: () =>
        h(ConfirmDialog as Component, {
          ...props,
          onDone: (result: unknown) => {
            resolve((result ?? fallback) as T);
            // Unmount after this event handler finishes, so the Dialog can
            // hand focus back to what had it.
            queueMicrotask(() => {
              app.unmount();
              host.remove();
            });
          },
        }),
    });
    app.mount(host);
  });
}

/** Ask a yes/no question. Resolves true on OK, false on Cancel, Escape or a backdrop click. */
export function confirmDialog(message: string, options: ConfirmOptions = {}): Promise<boolean> {
  return ask<boolean>(
    { message, title: options.title, confirmLabel: options.confirmLabel ?? "OK", cancelLabel: options.cancelLabel ?? "Cancel", danger: options.danger ?? false },
    false,
  );
}

/** Ask for one line of text. Resolves the text, or null when cancelled. */
export function promptDialog(message: string, initial = "", options: ConfirmOptions = {}): Promise<string | null> {
  return ask<string | null>(
    { message, title: options.title, confirmLabel: options.confirmLabel ?? "OK", cancelLabel: options.cancelLabel ?? "Cancel", danger: false, input: initial },
    null,
  );
}

/** Tell the user something; resolves when they dismiss it. */
export async function alertDialog(message: string, options: ConfirmOptions = {}): Promise<void> {
  await ask<boolean>({ message, title: options.title, confirmLabel: options.confirmLabel ?? "OK", cancelLabel: null, danger: false }, false);
}
