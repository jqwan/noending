import type { ITheme, Terminal } from "@xterm/xterm";

// ANSI colors need separate palettes so colored CLI output remains legible
// against both app surfaces. Explicit RGB colors from an Agent stay its own.
const palettes = {
  light: {
    black: "#24292f", red: "#cf222e", green: "#116329", yellow: "#9a6700",
    blue: "#0550ae", magenta: "#8250df", cyan: "#096b72", white: "#57606a",
    brightBlack: "#6e7781", brightRed: "#a40e26", brightGreen: "#1a7f37", brightYellow: "#7d4e00",
    brightBlue: "#0969da", brightMagenta: "#6639ba", brightCyan: "#075e68", brightWhite: "#24292f",
  },
  dark: {
    black: "#1e1e1c", red: "#e07a6a", green: "#5cb377", yellow: "#d9a04a",
    blue: "#7d92ff", magenta: "#c792ea", cyan: "#70c5cf", white: "#b5b5b0",
    brightBlack: "#8a8a86", brightRed: "#f59b8d", brightGreen: "#8bd49c", brightYellow: "#f2c879",
    brightBlue: "#a7b5ff", brightMagenta: "#ddafff", brightCyan: "#9edee5", brightWhite: "#ececea",
  },
};

export function readTerminalTheme(): ITheme {
  const root = document.documentElement;
  const dark = root.dataset.theme === "dark" ||
    (root.dataset.theme !== "light" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  const style = getComputedStyle(root);
  const token = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
  const background = token("--bg-app", dark ? "#141413" : "#ffffff");
  const foreground = token("--text-primary", dark ? "#ececea" : "#111111");
  return {
    ...palettes[dark ? "dark" : "light"],
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionForeground: foreground,
    selectionBackground: token("--accent-wash", dark ? "#242c4e" : "#eef1ff"),
    selectionInactiveBackground: token("--bg-active", dark ? "#2d2d2b" : "#e6e6e3"),
  };
}

/** Lives with the cached xterm, including while its view is detached. */
export function observeTerminalTheme(term: Terminal): () => void {
  const update = () => { term.options.theme = readTerminalTheme(); };
  update();
  const observer = new MutationObserver(update);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  media?.addEventListener("change", update);
  return () => {
    observer.disconnect();
    media?.removeEventListener("change", update);
  };
}
