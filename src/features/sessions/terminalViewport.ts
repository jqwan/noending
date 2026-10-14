import type { Terminal } from "@xterm/xterm";

/** xterm 6.0's viewport canvas height is rounded, but scrollHeight is the
 * unrounded cell height × lines. At 125%/14px, 31 rows occupy 546px while
 * 31 × 17.6 = 545.6. The bottom then clamps from 88000 to 87999.6, and every
 * line toggles scroll state and scrollbar timers even after history is full.
 * Match the range to the actual viewport plus the scrollable buffer rows.
 * This guarded private seam is isolated to the pinned xterm 6.0 Windows
 * integration; FitAddon also reads this renderer's dimensions. */
export function connectTerminalViewport(term: Terminal): () => void {
  type Dimensions = { height?: number; scrollHeight?: number };
  type Scrollable = { setScrollDimensions: (dimensions: Dimensions) => void };
  const core = (term as unknown as { _core?: {
    _viewport?: { _scrollableElement?: Scrollable };
    _renderService?: { dimensions: { css: { cell: { height: number } } } };
  } })._core;
  const scrollable = core?._viewport?._scrollableElement;
  if (!scrollable || !core?._renderService) {
    console.warn("Terminal viewport adaptation unavailable for this xterm build");
    return () => {};
  }
  const original = scrollable.setScrollDimensions;
  const adapted = (dimensions: Dimensions) => {
    const cell = core._renderService?.dimensions.css.cell.height ?? 0;
    if (dimensions.height && dimensions.scrollHeight !== undefined && cell > 0) {
      dimensions = { ...dimensions, scrollHeight: Math.max(dimensions.scrollHeight,
        dimensions.height + term.buffer.active.baseY * cell) };
    }
    original.call(scrollable, dimensions);
  };
  scrollable.setScrollDimensions = adapted;
  return () => { if (scrollable.setScrollDimensions === adapted) scrollable.setScrollDimensions = original; };
}
