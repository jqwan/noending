import { listen } from "@tauri-apps/api/event";
import type { TerminalOutput, TerminalSnapshot } from "../../types";

function decodeBase64(data: string): Uint8Array {
  return Uint8Array.from(atob(data), char => char.charCodeAt(0));
}

/** Subscribe before attach, buffer until xterm exists, and skip snapshot overlap.
 * In particular, Windows ConPTY's initial cursor query must never be dropped.
 * Ownership transfers to the cached terminal when replay starts. */
export function subscribeTerminalStream(id: string) {
  let disposed = false;
  let offset = 0;
  let write: ((data: Uint8Array) => void) | null = null;
  let onExit: (() => void) | null = null;
  let pending: TerminalOutput[] = [];
  const stops: (() => void)[] = [];

  const deliver = (output: TerminalOutput) => {
    if (disposed) return;
    if (!write) { pending.push(output); return; }
    const bytes = decodeBase64(output.data);
    const skip = Math.max(0, offset - output.offset);
    if (skip >= bytes.length) return;
    offset = output.offset + bytes.length;
    write(bytes.subarray(skip));
  };
  const track = (subscription: Promise<() => void>) => subscription.then(stop => {
    if (disposed) stop();
    else stops.push(stop);
  });

  const stream = {
    retained: false,
    exited: false,
    ready: Promise.resolve(),
    replay(snapshot: TerminalSnapshot, sink: (data: Uint8Array) => void, exit: () => void) {
      if (disposed) return;
      stream.retained = true;
      offset = snapshot.output_offset;
      write = sink;
      onExit = exit;
      stream.exited ||= !snapshot.live;
      // Register input and cache ownership before replay: queries can emit replies.
      sink(decodeBase64(snapshot.scrollback));
      pending.sort((a, b) => a.offset - b.offset).forEach(deliver);
      pending = [];
      if (stream.exited) exit();
    },
    dispose() {
      disposed = true;
      stops.splice(0).forEach(stop => stop());
      pending = [];
      write = null;
      onExit = null;
    },
  };
  stream.ready = Promise.all([
    track(listen<TerminalOutput>(`terminal-output://${id}`, event => deliver(event.payload))),
    track(listen<{ exit_code: number | null }>(`terminal-exit://${id}`, () => {
      if (disposed) return;
      stream.exited = true;
      onExit?.();
    })),
  ]).then(() => {}).catch(error => {
    stream.dispose();
    throw error;
  });
  return stream;
}

export type TerminalStream = ReturnType<typeof subscribeTerminalStream>;
