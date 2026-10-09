import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { waitFor } from "@testing-library/react";
import { connectTerminalInput } from "./terminalInput";

// Use the actual xterm renderer/parser and mouse encoder. jsdom supplies no
// font layout, canvas or media queries, so give it a 10×20 CSS-pixel cell.
vi.hoisted(() => vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null));
let term: Terminal;
let stop: (() => void) | undefined;
let screen: HTMLElement;
const send = vi.fn();

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  vi.stubGlobal("matchMedia", () => ({
    matches: false, addListener() {}, removeListener() {},
    addEventListener() {}, removeEventListener() {},
  }));
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(320);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(20);
  term = new Terminal({ cols: 80, rows: 24 });
  const host = document.createElement("div");
  document.body.append(host);
  term.open(host);
  screen = host.querySelector<HTMLElement>(".xterm-screen")!;
  screen.style.padding = "0px";
  vi.spyOn(screen, "getBoundingClientRect").mockReturnValue({
    x: 0, y: 0, left: 0, top: 0, right: 800, bottom: 480,
    width: 800, height: 480, toJSON() {},
  });
  stop = connectTerminalInput(term, send);
});

afterEach(() => {
  stop?.();
  stop = undefined;
  term.dispose();
  document.body.replaceChildren();
  send.mockReset();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function write(data: string) {
  return new Promise<void>((resolve) => term.write(data, resolve));
}

function wheel(deltaY: number, extra: WheelEventInit = {}) {
  const event = new WheelEvent("wheel", {
    bubbles: true, cancelable: true, deltaY, clientX: 25, clientY: 45, ...extra,
  });
  screen.dispatchEvent(event);
  return event;
}

it("preserves small trackpad movement and large distances in one SGR input per wheel event", async () => {
  await write("\x1b[?1049h\x1b[?1000h\x1b[?1006h");
  // Four sub-cell movements form one row; a 60px movement is three rows.
  for (let i = 0; i < 3; i++) expect(wheel(5).defaultPrevented).toBe(true);
  expect(send).not.toHaveBeenCalled();
  wheel(5);
  expect(send.mock.calls).toEqual([["\x1b[<65;3;3M"]]);
  wheel(60);
  expect(send.mock.calls[1]).toEqual(["\x1b[<65;3;3M".repeat(3)]);
  wheel(-40);
  expect(send.mock.calls[2]).toEqual(["\x1b[<64;3;3M".repeat(2)]);
  wheel(15);
  wheel(-20);
  expect(send.mock.calls[3]).toEqual(["\x1b[<64;3;3M"]);
});

it("retains mouse modifiers without applying the TUI's acceleration a second time", async () => {
  await write("\x1b[?1000h\x1b[?1006h");
  wheel(3, { deltaMode: WheelEvent.DOM_DELTA_LINE, altKey: true });
  expect(send.mock.calls).toEqual([["\x1b[<73;3;3M".repeat(3)]]);
  wheel(-2, { deltaMode: WheelEvent.DOM_DELTA_LINE, ctrlKey: true });
  expect(send.mock.calls[1]).toEqual(["\x1b[<80;3;3M".repeat(2)]);
});

it("uses cursor sequences in the alternate screen without mouse reporting, including application mode", async () => {
  await write("\x1b[?1049h");
  wheel(3, { deltaMode: WheelEvent.DOM_DELTA_LINE });
  expect(send.mock.calls).toEqual([["\x1b[B".repeat(3)]]);
  await write("\x1b[?1h");
  wheel(-1, { deltaMode: WheelEvent.DOM_DELTA_PAGE });
  expect(send.mock.calls[1]).toEqual(["\x1bOA".repeat(24)]);
});

it("leaves ordinary scrollback to xterm, clears pending movement on buffer switches, and disposes input forwarding", async () => {
  await write("\x1b[?1049h");
  wheel(15);
  await write("\x1b[?1049l" + "history\r\n".repeat(40));
  // A normal-buffer wheel must not be sent to the running shell as arrow keys.
  wheel(-60);
  expect(send).not.toHaveBeenCalled();
  await waitFor(() => expect(term.buffer.active.viewportY).toBeLessThan(term.buffer.active.baseY));
  await write("\x1b[?1049h");
  wheel(5);
  expect(send).not.toHaveBeenCalled();
  term.input("typed", true);
  expect(send.mock.calls).toEqual([["typed"]]);
  stop!();
  stop = undefined;
  term.input("after disposal", true);
  wheel(60);
  expect(send.mock.calls).toEqual([["typed"]]);
});
