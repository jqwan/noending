import type { Terminal } from "@xterm/xterm";

export function isMacTerminal(): boolean {
  return /Macintosh|Mac OS X/.test(navigator.userAgent);
}

/** Keep the measured WKWebView direct-commit workaround out of Chromium.
 * Composition itself remains owned by xterm; only WebKit's missing/duplicate
 * insertText events are reconciled. */
function connectWebkitIme(term: Terminal) {
  // Reconcile at the end of the native event chain, with at most one check.
  // input/keyup supply later changes; a fixed 120ms poll is unnecessary.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const later = () => {
    if (!armed || composing || timer !== undefined) return;
    timer = setTimeout(() => { timer = undefined; deliver(); }, 0);
  };
  let armed = false;
  let armValue = "";
  let xtermSent = "";
  let composing = false;
  let preedit = "";
  let literalSpace = false;
  let commit: { raw: string; text: string; value: string } | undefined;
  // 重复派发鉴别基线：上一次交付的载荷与其时的 textarea 值。
  let lastPayload = "";
  let lastPayloadValue: string | null = null;
  // 扣账交付：投入的文本先减去 xterm 在本窗口内已经发过的部分——
  // 无论这条文本来自 input 载荷还是 textarea 差分，都恰好交付一次。
  const deliverText = (text: string) => {
    if (!text) return;
    let residue = text;
    if (xtermSent) {
      if (residue.endsWith(xtermSent)) residue = residue.slice(0, residue.length - xtermSent.length);
    }
    if (residue) term.input(residue, true);
    xtermSent = "";
    armValue = term.textarea?.value ?? armValue;
  };
  const deliver = () => {
    if (!armed || composing || !term.textarea) return;
    const now = term.textarea.value;
    if (now === armValue || !now.startsWith(armValue)) return;
    deliverText(now.slice(armValue.length));
  };
  const armGesture = () => {
    deliver(); // 上一窗口若有未交的残余，先交掉、基线推进
    armed = true;
    armValue = term.textarea?.value ?? "";
    xtermSent = "";
    // 鉴别基线只在 textarea 落上新内容时才推进：同载荷且值未变，跨窗口
    // 仍视为重复派发（重复派发的两份之间可能隔着一个 229 keydown）。
    if (term.textarea?.value !== lastPayloadValue) {
      lastPayload = "";
      lastPayloadValue = term.textarea?.value ?? null;
    }
    later();
  };
  const accounting = term.onData((d) => {
    if (armed) xtermSent += d;
  });
  const compositionStart = () => {
    composing = true;
    preedit = "";
    literalSpace = false;
    commit = undefined;
  };
  const compositionUpdate = (event: CompositionEvent) => { preedit = event.data; };
  const compositionEnd = (event: CompositionEvent) => {
    const raw = event?.data ?? "";
    const textarea = term.textarea;
    // macOS Pinyin can commit its displayed syllable separators when the
    // input source changes. Remove only those preedit separators, never
    // whitespace in ordinary input, a paste, or a converted candidate.
    const text = !literalSpace && raw === preedit && /^[a-z]+(?: [a-z]+)+$/i.test(raw)
      ? raw.replace(/ /g, "") : raw;
    if (textarea && text !== raw && textarea.value.endsWith(raw)) {
      textarea.value = textarea.value.slice(0, -raw.length) + text;
    }
    commit = { raw, text, value: textarea?.value ?? "" };
    composing = false;
    // xterm's compositionend handler owns this commit. The diff fallback
    // must not send the same textarea change before/after its deferred send.
    armed = false;
    armValue = textarea?.value ?? "";
    xtermSent = "";
  };
  term.textarea?.addEventListener("compositionstart", compositionStart);
  term.textarea?.addEventListener("compositionupdate", compositionUpdate);
  // Normalize before xterm reads the final textarea value.
  term.element?.addEventListener("compositionend", compositionEnd, true);
  // input 的唯一收口（捕获层，先于 xterm 挂在 textarea 上的监听）。
  const input = (e: Event) => {
    const ie = e as InputEvent;
    if (ie.inputType !== "insertText") return;
    if (!ie.data) { later(); return; }
    if (composing || (commit && term.textarea?.value === commit.value &&
      (ie.data === commit.raw || ie.data === commit.text))) {
      // A composition's final input event is not a second commit. Keep it
      // away from xterm's independent emoji/input path as well as our shim.
      e.stopImmediatePropagation();
      return;
    }
    if (!armed) return;
    // 武装窗口内 xterm 的 input 通路退场：_keyDownSeen 已被上一键的
    // keyup 清零时，它会对同一份 ev.data 再交付一次。
    e.stopImmediatePropagation();
    if (ie.data === lastPayload && term.textarea?.value === lastPayloadValue) return;
    lastPayload = ie.data;
    lastPayloadValue = term.textarea?.value ?? null;
    // input 载荷是权威文本：textarea 可能被随时清空，差分读不到，
    // 但载荷永远带着本次插入的内容（首键丢失的正是这一份）。
    deliverText(ie.data);
  };
  term.element?.addEventListener("input", input, true);

  return {
    key(event: KeyboardEvent) {
      if (event.type === "keyup") { later(); return true; }
      if (event.type !== "keydown") return true;
      const modifier = ["Shift", "Control", "Alt", "Meta"].includes(event.key);
      if (composing && (modifier || event.keyCode === 20 || event.key === "Fn" || event.key === "Unidentified" ||
        (event.key === " " && (event.ctrlKey || event.altKey || event.metaKey)))) {
        // Let the OS switch input sources. Passing these keys to xterm can
        // prematurely finalize the preedit before the native commit arrives.
        return false;
      }
      if (composing && event.key === " ") literalSpace = true;
      if (!composing && !modifier && event.keyCode !== 20 && event.key !== "Fn" && event.key !== "Unidentified") {
        commit = undefined;
      }
      if (event.keyCode === 229) {
        if (!composing) armGesture();
        return false;
      }
      if (modifier) {
        armGesture();
        return true;
      }
      armed = false;
      return true;
    },
    dispose() {
      clearTimeout(timer);
      accounting.dispose();
      term.textarea?.removeEventListener("compositionstart", compositionStart);
      term.textarea?.removeEventListener("compositionupdate", compositionUpdate);
      term.element?.removeEventListener("compositionend", compositionEnd, true);
      term.element?.removeEventListener("input", input, true);
    },
  };
}

/** Clipboard text always enters xterm.paste, including native menu events.
 * This preserves newline normalization and the application's bracketed-paste
 * mode instead of pretending a paste is a series of typed commands. */
export function connectTerminalKeyboard(term: Terminal, options: {
  mac: boolean;
  writable: () => boolean;
  readClipboard: () => Promise<string | null>;
  copy: (text: string) => Promise<unknown>;
}): () => void {
  const ime = options.mac ? connectWebkitIme(term) : undefined;
  let disposed = false;
  let pasteQueue = Promise.resolve();
  const enqueuePaste = (pending: Promise<string | null>) => {
    // Start reading at the gesture, but deliver in gesture order, including
    // menu paste. Observe rejections immediately while another read is pending.
    const result = pending.catch(() => null);
    pasteQueue = pasteQueue.then(async () => {
      if (disposed || !options.writable()) return;
      const text = await result;
      if (text && !disposed && options.writable()) term.paste(text);
    }).catch(() => {});
  };
  const clipboardPaste = () => enqueuePaste(options.readClipboard());
  term.attachCustomKeyEventHandler(event => {
    if (disposed || !options.writable()) return false;
    if (ime?.key(event) === false) return false;
    if (event.type !== "keydown" || event.isComposing) return true;
    const key = event.key.toLowerCase();
    const copyKey = options.mac
      ? event.metaKey && !event.ctrlKey && !event.altKey && key === "c"
      : event.ctrlKey && !event.altKey && key === "c" && (event.shiftKey || term.hasSelection());
    if (copyKey && term.hasSelection()) {
      event.preventDefault();
      void options.copy(term.getSelection()).catch(() => {});
      return false;
    }
    const pasteKey = options.mac
      ? event.metaKey && !event.ctrlKey && !event.altKey && key === "v"
      : event.ctrlKey && event.shiftKey && !event.altKey && key === "v";
    if (pasteKey) {
      // Cancel the browser's default paste as well as xterm's key path.
      event.preventDefault();
      if (!event.repeat) clipboardPaste();
      return false;
    }
    return true;
  });
  const paste = (event: ClipboardEvent) => {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (disposed || !options.writable()) return;
    const text = event.clipboardData?.getData("text/plain");
    if (text) enqueuePaste(Promise.resolve(text));
    else if (!event.clipboardData) clipboardPaste();
  };
  term.element?.addEventListener("paste", paste, true);
  return () => {
    disposed = true;
    ime?.dispose();
    term.element?.removeEventListener("paste", paste, true);
  };
}
