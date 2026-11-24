// A scrolled element loses its scroll offset when <keep-alive> parks its view:
// the DOM is detached from the page, and a detached element reads 0 again.
// Call this in the component that owns the scroller and its position comes
// back when the view is shown again.
//
// The offset is tracked from `scroll` events rather than read in
// `onDeactivated`, because Vue runs that hook after the DOM has been moved.

import { nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, type Ref } from "vue";

export function useKeepScroll(el: Ref<HTMLElement | null>): void {
  let top = 0;
  let left = 0;
  let parked = false;

  const remember = () => {
    const node = el.value;
    if (parked || node === null || !node.isConnected) return;
    top = node.scrollTop;
    left = node.scrollLeft;
  };

  onMounted(() => el.value?.addEventListener("scroll", remember, { passive: true }));
  onBeforeUnmount(() => el.value?.removeEventListener("scroll", remember));
  onDeactivated(() => {
    parked = true;
  });
  onActivated(() => {
    parked = false;
    void nextTick(() => {
      const node = el.value;
      if (node === null) return;
      if (node.scrollTop !== top) node.scrollTop = top;
      if (node.scrollLeft !== left) node.scrollLeft = left;
    });
  });
}
