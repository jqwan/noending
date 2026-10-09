import { waitFor } from "@testing-library/react";
import { afterAll, afterEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { observeTerminalTheme, readTerminalTheme } from "./terminalTheme";

// These tests exercise the real escape parser without opening a renderer.
const canvas = vi.hoisted(() => vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null));
afterAll(() => canvas.mockRestore());

const terminals: Terminal[] = [];
function terminal() {
  const term = new Terminal();
  terminals.push(term);
  return term;
}

afterEach(() => {
  terminals.splice(0).forEach((term) => term.dispose());
  delete document.documentElement.dataset.theme;
  document.documentElement.removeAttribute("style");
  vi.unstubAllGlobals();
});

function systemTheme(dark: boolean) {
  const media = new EventTarget() as EventTarget & { matches: boolean };
  media.matches = dark;
  vi.stubGlobal("matchMedia", vi.fn(() => media));
  return media;
}

function write(term: Terminal, data: string) {
  return new Promise<void>((resolve) => term.write(data, resolve));
}

async function setAppTheme(theme: "light" | "dark") {
  document.documentElement.dataset.theme = theme;
  // MutationObserver runs before this microtask, so absence checks cannot
  // pass simply because the theme update has not been processed yet.
  await new Promise<void>((resolve) => queueMicrotask(resolve));
}

it("answers native appearance queries and preserves other terminal mode handling", async () => {
  document.documentElement.dataset.theme = "light";
  const term = terminal();
  const replies: string[] = [];
  const nativeReplies: string[] = [];
  term.onData((data) => nativeReplies.push(data));
  const stop = observeTerminalTheme(term, (data) => replies.push(data));
  try {
    await write(term, "\x1b[?2031$p\x1b[?996n\x1b[?2031;2004h\x1b[?2031$p");
    expect(replies).toEqual(["\x1b[?2031;2$y", "\x1b[?997;2n", "\x1b[?2031;1$y"]);
    expect(term.modes.bracketedPasteMode).toBe(true);
    replies.length = 0;
    await write(term, "\x1b[?2031;2004l\x1b[?2031$p\x1b[2;3H\x1b[?6n");
    expect(replies).toEqual(["\x1b[?2031;2$y"]);
    expect(term.modes.bracketedPasteMode).toBe(false);
    expect(nativeReplies).toEqual(["\x1b[?2;3R"]);
    document.documentElement.dataset.theme = "dark";
    await write(term, "\x1b[?996n");
    expect(replies[replies.length - 1]).toBe("\x1b[?997;1n");
  } finally {
    stop();
  }
});

it("reports palette changes only to subscribed applications, until reset or disposed", async () => {
  document.documentElement.dataset.theme = "light";
  const media = systemTheme(false);
  const term = terminal();
  const replies: string[] = [];
  const stop = observeTerminalTheme(term, (data) => replies.push(data));
  try {
    await setAppTheme("dark");
    expect(replies).toEqual([]);
    await write(term, "\x1b[?2031h");
    await setAppTheme("light");
    expect(replies).toEqual(["\x1b[?997;2n"]);
    media.dispatchEvent(new Event("change"));
    expect(replies).toEqual(["\x1b[?997;2n"]);
    await setAppTheme("dark");
    expect(replies).toEqual(["\x1b[?997;2n", "\x1b[?997;1n"]);
    await write(term, "\x1b[?2031l");
    await setAppTheme("light");
    expect(replies).toHaveLength(2);
    await write(term, "\x1b[?2031h\x1bc\x1b[?2031$p");
    expect(replies[replies.length - 1]).toBe("\x1b[?2031;2$y");
    await setAppTheme("dark");
    expect(replies).toHaveLength(3);
  } finally {
    stop();
  }
  await write(term, "\x1b[?2031$p\x1b[?996n");
  expect(replies).toHaveLength(3);
});

it("uses the app's resolved surface, text and selection tokens", () => {
  const root = document.documentElement;
  root.style.setProperty("--bg-app", "#fafafa");
  root.style.setProperty("--text-primary", "#222222");
  root.style.setProperty("--accent-wash", "#ddddff");
  const theme = readTerminalTheme();
  expect(theme.background).toBe("#fafafa");
  expect(theme.foreground).toBe("#222222");
  expect(theme.cursor).toBe(theme.foreground);
  expect(theme.cursorAccent).toBe(theme.background);
  expect(theme.selectionBackground).toBe("#ddddff");
});

it("updates existing terminals on app changes and stops observing when disposed", async () => {
  document.documentElement.dataset.theme = "light";
  const term = terminal();
  const stop = observeTerminalTheme(term, vi.fn());
  try {
    expect(term.options.theme?.background).toBe("#ffffff");
    document.documentElement.dataset.theme = "dark";
    await waitFor(() => expect(term.options.theme?.background).toBe("#141413"));
  } finally {
    stop();
  }
  const held = term.options.theme;
  document.documentElement.dataset.theme = "light";
  await new Promise<void>((resolve) => queueMicrotask(resolve));
  expect(term.options.theme).toBe(held);
});

it("follows system changes only while the app uses the system theme", async () => {
  const media = systemTheme(false);
  const term = terminal();
  const stop = observeTerminalTheme(term, vi.fn());
  try {
    expect(term.options.theme?.background).toBe("#ffffff");
    const light = term.options.theme;
    media.matches = true;
    media.dispatchEvent(new Event("change"));
    expect(term.options.theme?.background).toBe("#141413");
    expect(term.options.theme?.foreground).not.toBe(light?.foreground);
    expect(term.options.theme?.blue).not.toBe(light?.blue);
    document.documentElement.dataset.theme = "light";
    await waitFor(() => expect(term.options.theme?.background).toBe("#ffffff"));
    media.dispatchEvent(new Event("change"));
    expect(term.options.theme?.background).toBe("#ffffff");
    media.matches = false;
    document.documentElement.dataset.theme = "dark";
    await waitFor(() => expect(term.options.theme?.background).toBe("#141413"));
    media.dispatchEvent(new Event("change"));
    expect(term.options.theme?.background).toBe("#141413");
    media.matches = true;
    delete document.documentElement.dataset.theme;
    await waitFor(() => expect(term.options.theme?.background).toBe("#141413"));
  } finally {
    stop();
  }
  const held = term.options.theme;
  media.matches = false;
  media.dispatchEvent(new Event("change"));
  expect(term.options.theme).toBe(held);
});
