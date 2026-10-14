import type { Terminal } from "@xterm/xterm";

type Cursor = { x: number; y: number; cols: number; rows: number };

/** System ConPTY can omit hidden cursor moves. Use its actual viewport
 * position solely for IME anchoring; never change the VT buffer or input. */
export function connectWindowsComposition(term: Terminal, query: () => Promise<Cursor | null>): () => void {
  const element = term.element!;
  const textarea = term.textarea!;
  const screen = element.querySelector<HTMLElement>(".xterm-screen")!;
  let disposed = false;
  let composing = false;
  let generation = 0;
  let cached: Cursor | null = null;
  let pending: Promise<void> | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastQuery = 0;
  const apply = () => {
    delete element.dataset.nativeComposition;
    if (!composing || !cached || cached.cols !== term.cols || cached.rows !== term.rows) return;
    const rect = screen.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    element.style.setProperty("--terminal-ime-x", `${Math.min(cached.x, term.cols - 1) * rect.width / term.cols}px`);
    element.style.setProperty("--terminal-ime-y", `${Math.min(cached.y, term.rows - 1) * rect.height / term.rows}px`);
    element.dataset.nativeComposition = "true";
  };
  const refresh = () => {
    if (disposed || !composing || !element.isConnected || document.activeElement !== textarea) return;
    if (pending) return pending;
    lastQuery = performance.now();
    const current = generation;
    pending = query().then(cursor => {
      if (!disposed && current === generation) { cached = cursor; apply(); }
    }).catch(error => {
      if (current === generation) { cached = null; apply(); }
      console.warn("Terminal IME cursor query failed", error);
    }).finally(() => {
      pending = undefined;
      if (!disposed && composing && current !== generation) schedule();
    });
    return pending;
  };
  // Native queries spawn a helper. Restrict them to active composition;
  // ordinary Agent output must not launch helper processes.
  const schedule = () => {
    clearTimeout(timer);
    if (!composing) return;
    timer = setTimeout(() => void refresh(), Math.max(30, 250 - (performance.now() - lastQuery)));
  };
  const start = () => { composing = true; generation++; cached = null; apply(); void refresh(); };
  const update = () => apply();
  const end = () => { composing = false; generation++; clearTimeout(timer); delete element.dataset.nativeComposition; };
  const output = term.onWriteParsed(schedule);
  const resize = term.onResize(apply);
  textarea.addEventListener("compositionstart", start);
  textarea.addEventListener("compositionupdate", update);
  textarea.addEventListener("compositionend", end);
  return () => {
    disposed = true;
    clearTimeout(timer);
    output.dispose();
    resize.dispose();
    textarea.removeEventListener("compositionstart", start);
    textarea.removeEventListener("compositionupdate", update);
    textarea.removeEventListener("compositionend", end);
    delete element.dataset.nativeComposition;
    element.style.removeProperty("--terminal-ime-x");
    element.style.removeProperty("--terminal-ime-y");
  };
}
