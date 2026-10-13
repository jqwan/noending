import { useEffect } from "react";

/** Keep macOS native scrollbars; reveal Windows scrollbars during scrolling. */
export function useAutoHideScrollbars() {
  useEffect(() => {
    if (!/Windows/.test(navigator.userAgent)) return;

    const root = document.documentElement;
    const timers = new Map<HTMLElement, number>();
    root.setAttribute("data-auto-hide-scrollbars", "");

    const onScroll = (event: Event) => {
      const element = event.target;
      // xterm already manages its own overlay scrollbar and fade-out.
      if (!(element instanceof HTMLElement) || element.closest(".xterm")) return;

      window.clearTimeout(timers.get(element));
      element.setAttribute("data-scrollbar-active", "");
      timers.set(element, window.setTimeout(() => {
        element.removeAttribute("data-scrollbar-active");
        timers.delete(element);
      }, 1000));
    };

    // Capture includes nested panes, textareas and portalled dialogs without
    // changing their wheel, keyboard or native scrollbar-drag handling.
    document.addEventListener("scroll", onScroll, { capture: true, passive: true });
    return () => {
      document.removeEventListener("scroll", onScroll, true);
      root.removeAttribute("data-auto-hide-scrollbars");
      for (const [element, timer] of timers) {
        window.clearTimeout(timer);
        element.removeAttribute("data-scrollbar-active");
      }
    };
  }, []);
}
