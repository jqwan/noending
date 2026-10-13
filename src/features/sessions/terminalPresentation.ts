import type { ITerminalOptions, Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";

// VS Code 1.141.1 terminalConfiguration / TerminalFontMetrics defaults.
// Keep other platforms' presentation unchanged until tested on their WebView.
export function terminalOptions(mac: boolean): ITerminalOptions {
  return {
    fontFamily: mac ? "Menlo, Monaco, 'Courier New', monospace, AppleBraille"
      : "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    fontSize: mac ? 12 : 12.5,
    fontWeight: "normal",
    fontWeightBold: "bold",
    lineHeight: 1,
    letterSpacing: 0,
    cursorBlink: !mac,
    cursorInactiveStyle: "outline",
    macOptionIsMeta: false,
    scrollSensitivity: 1,
    fastScrollSensitivity: 5,
    // VS Code defaults to no animation. In particular, never smooth native
    // trackpad gestures a second time or queue an animation for TUI input.
    smoothScrollDuration: 0,
    scrollback: 5000,
    minimumContrastRatio: 4.5,
    ignoreBracketedPasteMode: false,
  };
}

/** A cached terminal owns the renderer, not the currently mounted React view.
 * Like VS Code, a lost WebGL context falls back to the DOM renderer without
 * rebuilding the terminal or replaying PTY output. */
export function createTerminalRenderer(term: Terminal) {
  let addon: WebglAddon | undefined;
  let loss: { dispose(): void } | undefined;
  const fallback = () => {
    loss?.dispose();
    loss = undefined;
    addon?.dispose();
    addon = undefined;
  };
  const renderer = {
    onDimensionsChange: null as (() => void) | null,
    dispose: fallback,
  };
  try {
    addon = new WebglAddon();
    term.loadAddon(addon);
    loss = addon.onContextLoss(() => {
      fallback();
      renderer.onDimensionsChange?.();
    });
  } catch {
    fallback();
  }
  return renderer;
}

/** Fit only visible terminals, at most once per frame. xterm owns DPR/font
 * measurement; FitAddon uses the active renderer's cell dimensions. */
export function attachTerminalLayout(
  term: Terminal, fit: FitAddon, container: HTMLElement,
  resize: (cols: number, rows: number) => void,
) {
  let disposed = false;
  let frame: number | undefined;
  let first = true;
  const refresh = () => {
    if (disposed || frame !== undefined) return;
    frame = requestAnimationFrame(() => {
      frame = undefined;
      if (disposed || !container.isConnected || container.clientWidth <= 0 || container.clientHeight <= 0) return;
      try {
        fit.fit();
        if (term.cols > 0 && term.rows > 0) resize(term.cols, term.rows);
        term.refresh(0, term.rows - 1);
        if (first) { first = false; term.focus(); }
      } catch {
        // A detached/unmeasurable view will be fitted on its next attachment.
      }
    });
  };
  const resume = () => {
    if (disposed || document.visibilityState === "hidden" || !container.isConnected || container.clientWidth <= 0 || container.clientHeight <= 0) return;
    term.clearTextureAtlas();
    refresh();
  };
  const observer = new ResizeObserver(refresh);
  observer.observe(container);
  window.addEventListener("resize", refresh);
  window.addEventListener("focus", resume);
  document.addEventListener("visibilitychange", resume);
  document.fonts?.addEventListener("loadingdone", resume);
  void document.fonts?.ready.then(() => { if (!disposed) resume(); });
  return {
    refresh,
    dispose() {
      disposed = true;
      if (frame !== undefined) cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", refresh);
      window.removeEventListener("focus", resume);
      document.removeEventListener("visibilitychange", resume);
      document.fonts?.removeEventListener("loadingdone", resume);
    },
  };
}
