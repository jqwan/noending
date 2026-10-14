import type { Terminal } from "@xterm/xterm";

/** One parser write in flight per terminal. Coalesce bytes that arrive while
 * parsing, without waiting for animation frames (hidden renderers still ACK).
 * Bytes are never decoded individually, preserving split UTF-8/VT sequences. */
export function createTerminalOutput(term: Terminal) {
  let disposed = false;
  let writing = false;
  const queue: { bytes: Uint8Array; done: () => void }[] = [];
  const pump = () => {
    if (disposed || writing || !queue.length) return;
    let length = 0;
    const entries: typeof queue = [];
    do {
      const entry = queue.shift()!;
      entries.push(entry);
      length += entry.bytes.length;
    } while (queue.length && length + queue[0].bytes.length <= 64 * 1024);
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const entry of entries) { bytes.set(entry.bytes, offset); offset += entry.bytes.length; }
    writing = true;
    term.write(bytes, () => {
      writing = false;
      if (disposed) return;
      entries.forEach(entry => entry.done());
      pump();
    });
  };
  return {
    write(bytes: Uint8Array, done: () => void = () => {}) {
      if (!bytes.length) { done(); return; }
      if (disposed) return;
      queue.push({ bytes, done });
      pump();
    },
    dispose() { disposed = true; queue.length = 0; },
  };
}
