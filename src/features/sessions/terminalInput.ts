import type { Terminal } from "@xterm/xterm";

export function connectTerminalInput(term: Terminal, send: (data: string, binary?: boolean) => void): () => void {
  // xterm handles scrollback, mouse reporting and alternate-screen fallback.
  const input = term.onData(data => send(data));
  // Legacy mouse encodings can contain bytes that must not be UTF-8 encoded.
  const binaryInput = term.onBinary(data => send(data, true));
  return () => { input.dispose(); binaryInput.dispose(); };
}
