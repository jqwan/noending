import type { Terminal } from "@xterm/xterm";

/** Preserve wheel distance in TUIs. xterm 6 otherwise emits at most one report
 * per DOM event, and dampens small trackpad deltas before deciding to emit it.
 * Feed normalized line events back through xterm so it still owns mouse
 * coordinates, modifiers and the application's selected protocol. */
export function connectTerminalInput(term: Terminal, send: (data: string) => void): () => void {
  const element = term.element!;
  const screen = element.querySelector<HTMLElement>(".xterm-screen");
  let forwarding = false;
  let batch: string | null = null;
  let remainder = 0;
  let previousMode = "";
  const input = term.onData((data) => {
    if (batch !== null) batch += data;
    else send(data);
  });

  const wheel = (event: WheelEvent) => {
    if (forwarding) return;
    const mouse = term.modes.mouseTrackingMode;
    const reportsWheel = mouse !== "none" && mouse !== "x10";
    const alternate = term.buffer.active.type === "alternate";
    const mode = `${mouse}/${alternate}`;
    if (mode !== previousMode) remainder = 0;
    previousMode = mode;
    if (!reportsWheel && !alternate) {
      remainder = 0;
      return; // Let xterm scroll its own history normally.
    }
    if (!event.deltaY || event.shiftKey) return;

    const rowHeight = (screen?.getBoundingClientRect().height ?? 0) / term.rows;
    if (event.deltaMode === WheelEvent.DOM_DELTA_PIXEL && rowHeight <= 0) return;
    let distance = event.deltaY;
    if (event.deltaMode === WheelEvent.DOM_DELTA_PIXEL) distance /= rowHeight;
    else if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) distance *= term.rows;
    distance *= term.options.scrollSensitivity ?? 1;
    // Mouse-aware TUIs own Alt/Ctrl acceleration; applying it here as well
    // would multiply their native speed a second time (e.g. Pi's Alt+wheel).
    if (!reportsWheel && (event.altKey || event.ctrlKey)) {
      distance *= term.options.fastScrollSensitivity ?? 5;
    }
    if (Math.sign(distance) !== Math.sign(remainder)) remainder = 0;
    remainder += distance;
    // Bound work for an unusually large wheel event, without queuing stale
    // scroll input after the user's gesture has stopped.
    const lines = Math.min(100, Math.abs(Math.trunc(remainder)));
    remainder %= 1;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!lines) return;

    forwarding = true;
    batch = "";
    try {
      for (let i = 0; i < lines; i++) {
        element.dispatchEvent(new WheelEvent("wheel", {
          bubbles: true, cancelable: true,
          deltaMode: WheelEvent.DOM_DELTA_LINE, deltaY: Math.sign(distance),
          clientX: event.clientX, clientY: event.clientY,
          altKey: event.altKey, ctrlKey: event.ctrlKey, metaKey: event.metaKey,
        }));
      }
    } finally {
      const data = batch;
      batch = null;
      forwarding = false;
      // A wheel event can produce many protocol reports, but needs only one
      // frontend/backend call. Keyboard input still goes through immediately.
      if (data) send(data);
    }
  };
  element.addEventListener("wheel", wheel, { capture: true, passive: false });
  return () => {
    element.removeEventListener("wheel", wheel, true);
    input.dispose();
  };
}
