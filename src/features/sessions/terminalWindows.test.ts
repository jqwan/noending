import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import { Terminal } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { connectWindowsClipboard } from "./terminalWindows";
import { connectTerminalLinks } from "./terminalLinks";
import { createTerminalOutput } from "./terminalOutput";

vi.hoisted(() => vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null));
let term: Terminal;
let stop: () => void;
const read = vi.fn<() => Promise<string | null>>();
const copy = vi.fn().mockResolvedValue(true);
let live = true;
const data: string[] = [];
beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addListener() {}, removeListener() {} }));
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(320);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(20);
  term = new Terminal({ cols: 80, rows: 24, allowProposedApi: true });
  const host = document.createElement("div");
  document.body.append(host);
  term.open(host);
  term.onData(text => data.push(text));
  live = true;
  read.mockReset().mockResolvedValue("你\nsecond");
  copy.mockReset().mockResolvedValue(true);
  data.length = 0;
  stop = connectWindowsClipboard(term, { read, copy, live: () => live });
});
afterEach(() => {
  stop(); term.dispose(); document.body.replaceChildren();
  vi.restoreAllMocks(); vi.unstubAllGlobals();
  localStorage.removeItem("noending.terminal.windows.screenReader");
});
const write = (text: string) => new Promise<void>(resolve => term.write(text, resolve));
it("leaves native window close available without delivering Alt+F4 to the Agent", () => {
  expect(key("F4", { altKey: true, keyCode: 115 }).defaultPrevented).toBe(false);
  expect(data).toEqual([]);
});

it("keeps Chromium composition submission single through xterm's native input path", async () => {
  term.focus();
  term.textarea!.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  term.textarea!.value = "你";
  term.textarea!.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "你" }));
  await new Promise(resolve => setTimeout(resolve, 0));
  term.textarea!.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "你" }));
  await waitFor(() => expect(data).toEqual(["你"]));
});
function key(key: string, modifiers: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key, ...modifiers });
  term.textarea!.dispatchEvent(event);
  return event;
}

it("pastes keyboard and menu text once, respecting bracketed paste and normalizing newlines", async () => {
  await write("\x1b[?2004h");
  expect(key("v", { ctrlKey: true }).defaultPrevented).toBe(true);
  key("V", { ctrlKey: true, shiftKey: true, repeat: true });
  await waitFor(() => expect(data).toEqual(["\x1b[200~你\rsecond\x1b[201~"]));
  expect(read).toHaveBeenCalledTimes(1);
  const menu = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(menu, "clipboardData", { value: { getData: () => "菜单\n文本" } });
  term.textarea!.dispatchEvent(menu);
  expect(menu.defaultPrevented).toBe(true);
  await waitFor(() => expect(data[1]).toBe("\x1b[200~菜单\r文本\x1b[201~"));
  await write("\x1b[?2004l");
  read.mockResolvedValue("normal\ntext");
  key("Insert", { shiftKey: true });
  await waitFor(() => expect(data[2]).toBe("normal\rtext"));
});

it("forwards native Ctrl+V for image-only clipboard and drops late pastes after exit/dispose", async () => {
  read.mockResolvedValue(null);
  key("v", { ctrlKey: true });
  await waitFor(() => expect(data).toEqual(["\x16"]));
  let resolve!: (text: string) => void;
  read.mockReturnValue(new Promise(done => { resolve = done; }));
  key("v", { ctrlKey: true });
  live = false;
  resolve("late");
  await Promise.resolve();
  expect(data).toEqual(["\x16"]);
});

it("copies wrapped selections, then leaves Ctrl+C without selection as interrupt", async () => {
  await write("one two\r\n中文");
  term.select(0, 0, 7);
  key("c", { ctrlKey: true });
  await waitFor(() => expect(copy).toHaveBeenCalledWith("one two"));
  await waitFor(() => expect(term.hasSelection()).toBe(false));
  key("c", { ctrlKey: true, keyCode: 67 });
  expect(data).toEqual(["\x03"]);
});

it("keeps Agent keys away from window shortcuts and lets Chromium input events deliver punctuation", () => {
  const app = vi.fn();
  window.addEventListener("keydown", app);
  key("k", { ctrlKey: true, keyCode: 75 });
  key("ArrowLeft", { altKey: true, keyCode: 37 });
  expect(app).not.toHaveBeenCalled();
  expect(data).toEqual(["\x0b", "\x1b[1;3D"]);
  window.removeEventListener("keydown", app);
  term.textarea!.dispatchEvent(new KeyboardEvent("keyup", { key: "k" }));
  for (const text of ["？", "？", "，", "."]) {
    term.textarea!.dispatchEvent(new InputEvent("input", { bubbles: true, data: text, inputType: "insertText" }));
  }
  expect(data.slice(2).join("")).toBe("？？，.");
});

it("uses Windows copy/paste right-click while preserving mouse-aware TUI right buttons", async () => {
  read.mockResolvedValue("right");
  term.element!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  await waitFor(() => expect(data).toEqual(["right"]));
  await write("\x1b[?1000h");
  term.element!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  expect(read).toHaveBeenCalledTimes(1);
});

it("offers keyboard-accessible paste and screen reading without stealing Agent keys", () => {
  key("F10", { shiftKey: true });
  const menu = document.querySelector('[role="menu"]')!;
  expect(menu).not.toBeNull();
  const toggle = menu.querySelector<HTMLButtonElement>('[role="menuitemcheckbox"]')!;
  expect(toggle.getAttribute("aria-checked")).toBe("false");
  toggle.click();
  expect(term.options.screenReaderMode).toBe(true);
  expect(document.querySelector('[role="menu"]')).toBeNull();
});

it("opens only HTTP(S) with Ctrl, leaving selections and TUI mouse interaction alone", () => {
  const open = vi.fn().mockResolvedValue(undefined);
  const stopLinks = connectTerminalLinks(term, false, open);
  const activate = (event: MouseEvent, text: string) => term.options.linkHandler!.activate(event, text,
    { start: { x: 1, y: 1 }, end: { x: 1, y: 1 } });
  activate(new MouseEvent("click"), "https://example.com");
  activate(new MouseEvent("click", { ctrlKey: true }), "command:workbench.action");
  activate(new MouseEvent("click", { ctrlKey: true }), "https://example.com/path");
  expect(open.mock.calls).toEqual([["https://example.com/path"]]);
  stopLinks();
});

it("parses coalesced split UTF-8 and VT bytes with modern emoji cell widths", async () => {
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  const output = createTerminalOutput(term);
  const bytes = new TextEncoder().encode("\x1b[1m你😀X\x1b[0m");
  const completed: number[] = [];
  for (let i = 0; i < bytes.length; i++) output.write(bytes.subarray(i, i + 1), () => completed.push(i));
  await waitFor(() => expect(completed).toHaveLength(bytes.length));
  const line = term.buffer.active.getLine(0)!;
  expect(line.translateToString(true)).toBe("你😀X");
  expect(line.getCell(0)!.getWidth()).toBe(2);
  expect(line.getCell(2)!.getWidth()).toBe(2);
  expect(line.getCell(4)!.getChars()).toBe("X");
  expect(line.getCell(4)!.isBold()).toBeTruthy();
  output.dispose();
});


it("delivers consecutive deliberate keyboard and menu pastes in gesture order", async () => {
  const reads: ((text: string) => void)[] = [];
  read.mockImplementation(() => new Promise(resolve => reads.push(resolve)));
  key("v", { ctrlKey: true });
  key("v", { ctrlKey: true });
  expect(reads).toHaveLength(2);
  const menu = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(menu, "clipboardData", { value: { getData: () => "third" } });
  term.textarea!.dispatchEvent(menu);
  reads[1]("second");
  await Promise.resolve();
  expect(data).toEqual([]);
  reads[0]("first");
  await waitFor(() => expect(data).toEqual(["first", "second", "third"]));
});

it("hides a cached terminal's menu on navigation and closes it when keyboard focus leaves", () => {
  key("F10", { shiftKey: true });
  expect(document.querySelector('[role="menu"]')).not.toBeNull();
  term.element!.dispatchEvent(new Event("terminal-view-detached"));
  const host = term.element!.parentElement!;
  host.remove(); document.body.append(host);
  expect(document.querySelector('[role="menu"]')).toBeNull();
  key("F10", { shiftKey: true });
  const outside = document.createElement("button"); document.body.append(outside);
  outside.focus();
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
