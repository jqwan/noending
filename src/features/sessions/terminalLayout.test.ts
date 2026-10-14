import { afterEach, expect, it, vi } from "vitest";
import { connectTerminalLayout } from "./terminalLayout";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

it("resizes once, keeps hidden dimensions, restores terminal focus, and serializes latest resize", async () => {
  const frames: FrameRequestCallback[] = [];
  let notify = () => {};
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  vi.stubGlobal("ResizeObserver", class {
    constructor(cb: () => void) { notify = cb; }
    observe() {} disconnect() {}
  });
  const container = document.createElement("div");
  const textarea = document.createElement("textarea");
  container.append(textarea); document.body.append(container);
  let visible = true;
  Object.defineProperties(container, {
    clientWidth: { get: () => visible ? 800 : 0 }, clientHeight: { get: () => visible ? 600 : 0 },
  });
  const term = { cols: 100, rows: 30, textarea, element: container, focus: () => textarea.focus() };
  let desired = [100, 30];
  const fit = { fit: () => { [term.cols, term.rows] = desired; } };
  let complete!: () => void;
  const resize = vi.fn().mockImplementationOnce(() => new Promise<void>(r => { complete = r; }))
    .mockResolvedValue(undefined);
  const stop = connectTerminalLayout(term, fit, container, resize, { cols: 80, rows: 24 });
  const frame = () => frames.shift()!(0);
  frame();
  expect(document.activeElement).toBe(textarea);
  expect(resize.mock.calls).toEqual([[100, 30]]);
  notify(); frame();
  expect(resize).toHaveBeenCalledTimes(1);
  visible = false; notify(); frame();
  expect([term.cols, term.rows]).toEqual([100, 30]);
  desired = [110, 35]; visible = true; notify(); frame();
  desired = [120, 40]; notify(); frame();
  expect(resize).toHaveBeenCalledTimes(1);
  complete(); await Promise.resolve(); await Promise.resolve();
  expect(resize.mock.calls).toEqual([[100, 30], [120, 40]]);
  window.dispatchEvent(new Event("blur")); textarea.blur();
  window.dispatchEvent(new Event("focus"));
  expect(document.activeElement).toBe(textarea);
  stop();
  textarea.blur(); window.dispatchEvent(new Event("focus"));
  expect(document.activeElement).not.toBe(textarea);
});
