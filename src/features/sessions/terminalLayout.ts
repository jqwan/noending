import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";

/** Mounted-view work only. Output/parser and renderer stay with the cache. */
export function connectTerminalLayout(term: Pick<Terminal, "cols" | "rows" | "element" | "textarea" | "focus">, fit: Pick<FitAddon, "fit">, container: HTMLElement,
  resize: (cols: number, rows: number) => Promise<unknown>, initial: { cols: number; rows: number },
): () => void {
  let disposed = false;
  let frame = 0;
  let visible = false;
  let restoreFocus = true;
  let sent = `${initial.cols}/${initial.rows}`;
  let desired = sent;
  let resizing = false;
  const flushSize = async () => {
    if (resizing) return;
    resizing = true;
    try {
      while (!disposed && desired !== sent) {
        const next = desired;
        const [cols, rows] = next.split("/").map(Number);
        await resize(cols, rows);
        sent = next;
      }
    } catch (error) { console.warn("Terminal resize failed", error); }
    finally { resizing = false; }
  };
  const layout = () => {
    frame = 0;
    if (disposed || !container.isConnected || container.clientWidth <= 0 || container.clientHeight <= 0) {
      visible = false;
      return;
    }
    fit.fit();
    desired = `${term.cols}/${term.rows}`;
    void flushSize();
    if (!visible && restoreFocus && !document.querySelector('[role="dialog"]')) term.focus();
    visible = true;
  };
  const schedule = () => { if (!frame && !disposed) frame = requestAnimationFrame(layout); };
  const blur = () => { restoreFocus = document.activeElement === term.textarea; };
  const focus = () => {
    schedule();
    if (restoreFocus && container.clientWidth > 0 && container.clientHeight > 0 &&
        !document.querySelector('[role="dialog"]')) term.focus();
  };
  const observer = new ResizeObserver(schedule);
  observer.observe(container);
  // xterm remeasures on DPR changes. A matching media query also refits even
  // when the CSS container size is unchanged after moving monitors.
  let density: MediaQueryList | undefined;
  const densityChanged = () => { watchDensity(); schedule(); };
  const watchDensity = () => {
    density?.removeEventListener("change", densityChanged);
    density = window.matchMedia?.(`(resolution: ${window.devicePixelRatio}dppx)`);
    density?.addEventListener("change", densityChanged);
  };
  watchDensity();
  window.addEventListener("resize", schedule);
  window.addEventListener("blur", blur);
  window.addEventListener("focus", focus);
  document.addEventListener("visibilitychange", schedule);
  document.fonts?.addEventListener("loadingdone", schedule);
  term.element?.addEventListener("terminal-renderer-changed", schedule);
  schedule();
  return () => {
    disposed = true;
    cancelAnimationFrame(frame);
    observer.disconnect();
    density?.removeEventListener("change", densityChanged);
    window.removeEventListener("resize", schedule);
    window.removeEventListener("blur", blur);
    window.removeEventListener("focus", focus);
    document.removeEventListener("visibilitychange", schedule);
    document.fonts?.removeEventListener("loadingdone", schedule);
    term.element?.removeEventListener("terminal-renderer-changed", schedule);
  };
}
