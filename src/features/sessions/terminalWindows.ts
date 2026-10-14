import type { Terminal } from "@xterm/xterm";

export const isWindowsTerminal = () => /Win/.test(navigator.platform);

/** WebView2 uses xterm's Chromium composition path. Do not intercept input or
 * composition events here: the WKWebView repair belongs only to macOS. */
export function connectWindowsClipboard(term: Terminal, clipboard: {
  read: () => Promise<string | null>;
  copy: (text: string) => Promise<unknown>;
  live: () => boolean;
}): () => void {
  const element = term.element!;
  let disposed = false;
  let pasteQueue = Promise.resolve();
  const accessibilityKey = "noending.terminal.windows.screenReader";
  const accessibilityEvent = "noending:terminal-accessibility";
  const updateAccessibility = () => { term.options.screenReaderMode = localStorage.getItem(accessibilityKey) === "true"; };
  updateAccessibility();
  window.addEventListener(accessibilityEvent, updateAccessibility);
  let menu: HTMLElement | undefined;
  const closeMenu = () => { menu?.remove(); menu = undefined; };
  const enqueuePaste = (pending: Promise<string | null>, nativeImageKey: boolean) => {
    // Read at each gesture, then deliver in gesture order. A second deliberate
    // paste is not a duplicate browser event and must never be discarded.
    const result = pending.catch(error => { console.warn("Terminal clipboard read failed", error); return undefined; });
    pasteQueue = pasteQueue.then(async () => {
      const text = await result;
      if (disposed || !clipboard.live() || !element.isConnected || text === undefined) return;
      if (text) term.paste(text);
      else if (nativeImageKey) term.input("\x16", true);
    }).catch(error => console.warn("Terminal paste failed", error));
  };
  const paste = (nativeImageKey = false) => {
    if (!disposed && clipboard.live() && element.isConnected) enqueuePaste(clipboard.read(), nativeImageKey);
  };
  const copy = (clear: boolean) => {
    const text = term.getSelection();
    void clipboard.copy(text).then(result => {
      if (!disposed && result !== false && clear && term.getSelection() === text) term.clearSelection();
    }).catch(error => console.warn("Terminal clipboard copy failed", error));
  };
  const showMenu = (x: number, y: number) => {
    closeMenu();
    menu = document.createElement("div");
    menu.className = "terminal-context-menu";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", "终端操作");
    menu.style.left = `${Math.max(0, Math.min(x, window.innerWidth - 240))}px`;
    menu.style.top = `${Math.max(0, Math.min(y, window.innerHeight - 140))}px`;
    const actions = [
      { label: "复制", disabled: !term.hasSelection(), run: () => copy(false) },
      { label: "粘贴", disabled: !clipboard.live(), run: () => void paste(true) },
      { label: "屏幕阅读支持", disabled: false, run: () => {
        localStorage.setItem(accessibilityKey, String(!term.options.screenReaderMode));
        window.dispatchEvent(new Event(accessibilityEvent));
      } },
    ];
    actions.forEach((action, index) => {
      const button = document.createElement("button");
      button.textContent = action.label;
      button.disabled = action.disabled;
      button.setAttribute("role", index === 2 ? "menuitemcheckbox" : "menuitem");
      if (index === 2) button.setAttribute("aria-checked", String(term.options.screenReaderMode));
      button.onclick = () => { action.run(); closeMenu(); term.focus(); };
      menu!.append(button);
    });
    menu.addEventListener("keydown", event => {
      event.stopPropagation();
      if (event.key === "Escape") { event.preventDefault(); closeMenu(); term.focus(); }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const buttons = [...menu!.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
      }
    });
    // Keep it in the mounted view, outside xterm's mouse/input handlers.
    element.parentElement?.append(menu);
    menu.addEventListener("focusout", event => {
      if (!menu?.contains(event.relatedTarget as Node | null)) closeMenu();
    });
    menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  };
  const dismissMenu = (event: Event) => { if (menu && !menu.contains(event.target as Node)) closeMenu(); };
  document.addEventListener("pointerdown", dismissMenu);
  term.attachCustomKeyEventHandler(event => {
    if (event.type !== "keydown" || event.isComposing || event.keyCode === 229) return true;
    const key = event.key.toLowerCase();
    // Let the native Windows window shortcut reach its default action.
    if (event.altKey && !event.ctrlKey && key === "f4") return false;
    if (key === "f6") return false;
    if (key === "contextmenu" || (key === "f10" && event.shiftKey)) {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      showMenu(rect.left + 12, rect.top + 12);
      return false;
    }
    if (event.ctrlKey && !event.altKey && !event.metaKey && key === "c" && term.hasSelection()) {
      event.preventDefault();
      copy(!event.shiftKey);
      return false;
    }
    if ((event.ctrlKey && !event.altKey && !event.metaKey && key === "v") ||
        (event.shiftKey && !event.ctrlKey && !event.altKey && key === "insert")) {
      event.preventDefault();
      if (!event.repeat) void paste(event.ctrlKey && !event.shiftKey);
      return false;
    }
    return true;
  });
  // Keyboard paste cancels the browser default action. Native menu paste has
  // no preceding keydown and is handled exactly once through xterm.paste.
  const onPaste = (event: ClipboardEvent) => {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!clipboard.live() || disposed || !element.isConnected) return;
    const text = event.clipboardData?.getData("text/plain");
    if (text) enqueuePaste(Promise.resolve(text), false);
    else paste(true);
  };
  const onContextMenu = (event: MouseEvent) => {
    // Mouse-reporting applications keep their native right button. Shift is
    // xterm's explicit override for selecting/copying in a mouse-aware TUI.
    if (!event.shiftKey && term.modes.mouseTrackingMode !== "none") {
      event.preventDefault();
      return;
    }
    if (event.shiftKey) {
      event.preventDefault(); showMenu(event.clientX, event.clientY); return;
    }
    event.preventDefault();
    if (term.hasSelection()) copy(true);
    else { term.focus(); void paste(true); }
  };
  const onKeyDown = (event: KeyboardEvent) => {
    // F6 provides a way out without taking Tab/Escape away from the Agent.
    if (event.key === "F6" && !event.isComposing) {
      event.preventDefault();
      element.closest(".main")?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    }
    // Prevent NoEnding's window-level navigation/palette shortcuts from also
    // executing after xterm has delivered the same key to the Agent.
    event.stopPropagation();
  };
  element.addEventListener("terminal-view-detached", closeMenu);
  element.addEventListener("paste", onPaste, true);
  element.addEventListener("contextmenu", onContextMenu);
  element.addEventListener("keydown", onKeyDown);
  return () => {
    disposed = true;
    closeMenu();
    window.removeEventListener(accessibilityEvent, updateAccessibility);
    document.removeEventListener("pointerdown", dismissMenu);
    element.removeEventListener("paste", onPaste, true);
    element.removeEventListener("contextmenu", onContextMenu);
    element.removeEventListener("keydown", onKeyDown);
    element.removeEventListener("terminal-view-detached", closeMenu);
  };
}
