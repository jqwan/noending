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

function isDarkTheme(): boolean {
  const root = document.documentElement;
  return root.dataset.theme === "dark" ||
    (root.dataset.theme !== "light" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
}

export function readTerminalTheme(): ITheme {
  const root = document.documentElement;
  const dark = isDarkTheme();
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
export function observeTerminalTheme(term: Terminal, respond: (data: string) => void): () => void {
  // xterm 6.0 answers OSC color queries, but lacks DEC mode 2031. Advertise
  // and implement it so native auto themes can re-query colors and redraw.
  // https://contour-terminal.org/vt-extensions/color-palette-update-notifications/
  let reporting = false;
  let previous = "";
  const report = () => respond(`\x1b[?997;${isDarkTheme() ? 1 : 2}n`);
  const handlers = [
    term.parser.registerCsiHandler({ prefix: "?", intermediates: "$", final: "p" }, (params) => {
      if (params.length !== 1 || params[0] !== 2031) return false;
      respond(`\x1b[?2031;${reporting ? 1 : 2}$y`);
      return true;
    }),
    term.parser.registerCsiHandler({ prefix: "?", final: "n" }, (params) => {
      if (params.length !== 1 || params[0] !== 996) return false;
      report();
      return true;
    }),
    ...(["h", "l"] as const).map((final) =>
      term.parser.registerCsiHandler({ prefix: "?", final }, (params) => {
        if (!params.includes(2031)) return false;
        reporting = final === "h";
        // Let xterm handle any other modes in the same sequence.
        return params.every((param) => param === 2031);
      })
    ),
    term.parser.registerEscHandler({ final: "c" }, () => {
      reporting = false;
      return false;
    }),
  ];
  const update = () => {
    const theme = readTerminalTheme();
    const palette = JSON.stringify(theme);
    term.options.theme = theme;
    if (reporting && previous !== palette) report();
    previous = palette;
  };
  update();
  const observer = new MutationObserver(update);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  media?.addEventListener("change", update);
  return () => {
    handlers.forEach((handler) => handler.dispose());
    observer.disconnect();
    media?.removeEventListener("change", update);
  };
}
