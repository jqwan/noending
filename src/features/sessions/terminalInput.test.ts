import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { waitFor } from "@testing-library/react";
import { connectTerminalInput } from "./terminalInput";
import { connectTerminalKeyboard } from "./terminalKeyboard";

// Use the actual xterm renderer/parser and mouse encoder. jsdom supplies no
// font layout, canvas or media queries, so give it a 10×20 CSS-pixel cell.
vi.hoisted(() => vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null));
let term: Terminal;
let stop: (() => void) | undefined;
let stopKeyboard: (() => void) | undefined;
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
  stop = connectTerminalInput(term, send, true);
});

afterEach(() => {
  stopKeyboard?.();
  stopKeyboard = undefined;
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

it("preserves Windows alternate-screen scrolling when an application has no mouse reports", async () => {
  await write("\x1b[?1049h");
  wheel(3, { deltaMode: WheelEvent.DOM_DELTA_LINE });
  expect(send.mock.calls).toEqual([["\x1b[B".repeat(3)]]);
  await write("\x1b[?1h");
  wheel(-1, { deltaMode: WheelEvent.DOM_DELTA_LINE });
  expect(send.mock.calls[1]).toEqual(["\x1bOA"]);
});

it("preserves the existing non-Windows alternate-screen fallback and acceleration", async () => {
  stop!();
  stop = connectTerminalInput(term, send);
  await write("\x1b[?1049h");
  term.options.scrollSensitivity = 2;
  term.options.fastScrollSensitivity = 5;
  wheel(1, { deltaMode: WheelEvent.DOM_DELTA_LINE, altKey: true });
  expect(send.mock.calls).toEqual([["\x1b[B".repeat(10)]]);
  wheel(1, { deltaMode: WheelEvent.DOM_DELTA_LINE, ctrlKey: true });
  expect(send.mock.calls[1]).toEqual(["\x1b[B".repeat(10)]);
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

it("preserves raw legacy mouse bytes beyond ASCII instead of UTF-8 encoding coordinates", async () => {
  term.resize(200, 24);
  await write("\x1b[?1000h");
  wheel(1, { deltaMode: WheelEvent.DOM_DELTA_LINE, clientX: 1005 });
  expect(send.mock.calls).toEqual([["\x1b[Ma\x85#", true]]);
});

function key(key: string, modifiers: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", {
    key, keyCode: key.toUpperCase().charCodeAt(0), bubbles: true, cancelable: true, ...modifiers,
  });
  term.textarea!.dispatchEvent(event);
  return event;
}

it("uses the application's bracketed-paste mode for keyboard and native menu text", async () => {
  stopKeyboard = connectTerminalKeyboard(term, {
    mac: true, writable: () => true,
    readClipboard: async () => "first\n中文\r\nlast", copy: vi.fn().mockResolvedValue(true),
  });
  await write("\x1b[?2004h");
  expect(key("v", { metaKey: true }).defaultPrevented).toBe(true);
  await waitFor(() => expect(send.mock.calls).toEqual([["\x1b[200~first\r中文\rlast\x1b[201~"]]));
  await write("\x1b[?2004l");
  const paste = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(paste, "clipboardData", { value: { getData: () => "menu\ntext" } });
  term.textarea!.dispatchEvent(paste);
  await waitFor(() => expect(send.mock.calls[1]).toEqual(["menu\rtext"]));
  expect(paste.defaultPrevented).toBe(true);
});

it("keeps Ctrl+C as interrupt on macOS with a selection and passes Agent paste keys through", async () => {
  const copy = vi.fn().mockResolvedValue(true);
  const readClipboard = vi.fn();
  stopKeyboard = connectTerminalKeyboard(term, { mac: true, writable: () => true, readClipboard, copy });
  await write("selected text");
  term.select(0, 0, 8);
  key("c", { metaKey: true });
  expect(copy).toHaveBeenCalledWith("selected");
  expect(send).not.toHaveBeenCalled();
  key("c", { ctrlKey: true });
  key("v", { ctrlKey: true });
  expect(send.mock.calls).toEqual([["\x03"], ["\x16"]]);
  expect(readClipboard).not.toHaveBeenCalled();
});

it("orders delayed clipboard and menu pastes and discards a read when the terminal is disposed", async () => {
  let resolve!: (text: string) => void;
  const pending = new Promise<string>(done => { resolve = done; });
  const readClipboard = vi.fn().mockReturnValue(pending);
  stopKeyboard = connectTerminalKeyboard(term, {
    mac: true, writable: () => true, readClipboard, copy: vi.fn().mockResolvedValue(true),
  });
  key("v", { metaKey: true });
  const paste = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(paste, "clipboardData", { value: { getData: () => "menu" } });
  term.textarea!.dispatchEvent(paste);
  expect(send).not.toHaveBeenCalled();
  resolve("keyboard");
  await waitFor(() => expect(send.mock.calls).toEqual([["keyboard"], ["menu"]]));
  let finish!: (text: string) => void;
  readClipboard.mockReturnValue(new Promise<string>(done => { finish = done; }));
  key("v", { metaKey: true });
  stopKeyboard();
  stopKeyboard = undefined;
  finish("late text");
  await Promise.resolve();
  await Promise.resolve();
  expect(send.mock.calls).toEqual([["keyboard"], ["menu"]]);
});

async function preedit(text: string, prefix = "") {
  term.textarea!.value = prefix;
  term.textarea!.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  term.textarea!.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: text }));
  term.textarea!.value = prefix + text;
  // The last IME key can precede the textarea update in another WebKit event order.
  key("s", { keyCode: 229 });
  await new Promise(resolve => setTimeout(resolve, 0));
}
function endComposition(text: string) {
  term.textarea!.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: text }));
}

it("commits unconverted Pinyin once without preedit separators when the input source changes", async () => {
  stopKeyboard = connectTerminalKeyboard(term, {
    mac: true, writable: () => true, readClipboard: async () => null, copy: vi.fn(),
  });
  await preedit("d h j s", "existing text ");
  expect(send).not.toHaveBeenCalled();
  key("CapsLock", { keyCode: 20 });
  key("Unidentified", { keyCode: 0 });
  // Some switches also expose a modifier before the native compositionend.
  key("Meta", { keyCode: 91, metaKey: true });
  expect(send).not.toHaveBeenCalled();
  endComposition("d h j s");
  key("CapsLock", { keyCode: 20 });
  key("Unidentified", { keyCode: 0 });
  term.textarea!.dispatchEvent(new InputEvent("input", {
    bubbles: true, inputType: "insertText", data: "d h j s",
  }));
  await waitFor(() => expect(send.mock.calls).toEqual([["dhjs"]]));
  await new Promise(resolve => setTimeout(resolve, 140));
  expect(send.mock.calls).toEqual([["dhjs"]]);
  expect(term.textarea!.value).toBe("existing text dhjs");
  key(" ", { keyCode: 32 });
  term.textarea!.dispatchEvent(new KeyboardEvent("keypress", {
    key: " ", keyCode: 32, charCode: 32, bubbles: true, cancelable: true,
  }));
  key("a");
  expect(send.mock.calls).toEqual([["dhjs"], [" "], ["a"]]);
});

it("keeps converted candidates and explicitly entered composition spaces", async () => {
  stopKeyboard = connectTerminalKeyboard(term, {
    mac: true, writable: () => true, readClipboard: async () => null, copy: vi.fn(),
  });
  await preedit("d h j s");
  term.textarea!.value = "对话结束";
  endComposition("对话结束");
  await waitFor(() => expect(send.mock.calls).toEqual([["对话结束"]]));
  await preedit("hello world");
  key(" ", { keyCode: 229 });
  endComposition("hello world");
  await waitFor(() => expect(send.mock.calls[1]).toEqual(["hello world"]));
});
