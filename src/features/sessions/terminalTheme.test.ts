import { waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import { observeTerminalTheme, readTerminalTheme } from "./terminalTheme";

afterEach(() => {
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
  const term = { options: {} } as Terminal;
  const stop = observeTerminalTheme(term);
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
  const term = { options: {} } as Terminal;
  const stop = observeTerminalTheme(term);
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
