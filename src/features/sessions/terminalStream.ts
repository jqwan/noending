import { listen } from "@tauri-apps/api/event";
import type { TerminalOutput, TerminalSnapshot } from "../../types";

function decodeBase64(data: string): Uint8Array {
  return Uint8Array.from(atob(data), char => char.charCodeAt(0));
}

/** Subscribe before attach, buffer until xterm exists, and skip snapshot overlap.
 * In particular, Windows ConPTY's initial cursor query must never be dropped.
 * Ownership transfers to the cached terminal when replay starts. */
export function subscribeTerminalStream(id: string, flow?: {
  acknowledge: (offset: number, consumerId: string) => Promise<unknown>;
  release: (consumerId: string) => Promise<unknown>;
}) {
  let disposed = false;
  const consumerId = flow ? crypto.randomUUID() : undefined;
  let attachment: Promise<TerminalSnapshot> | undefined;
  let offset = 0;
  let write: ((data: Uint8Array, done: () => void) => void) | null = null;
  let ackTimer: ReturnType<typeof setTimeout> | undefined;
  let parsedOffset = 0;
  let acknowledgedOffset = -1;
  let acknowledging = false;
  let retryDelay = 50;
  const scheduleAck = (delay = 0) => {
    if (!flow || disposed || acknowledging || ackTimer !== undefined) return;
    ackTimer = setTimeout(async () => {
      ackTimer = undefined;
      if (disposed) return;
      acknowledging = true;
      const position = parsedOffset;
      let failed = false;
      try {
        await flow.acknowledge(position, consumerId!);
        acknowledgedOffset = position;
        retryDelay = 50;
      } catch (error) {
        failed = true;
        console.warn("Terminal output acknowledgement failed", error);
      } finally {
        acknowledging = false;
        if (failed || parsedOffset > acknowledgedOffset) {
          scheduleAck(failed ? retryDelay : 0);
          if (failed) retryDelay = Math.min(1000, retryDelay * 2);
        }
      }
    }, delay);
  };
  const parsed = (position: number) => {
    parsedOffset = Math.max(parsedOffset, position);
    scheduleAck();
  };
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
    const end = offset;
    write(bytes.subarray(skip), () => parsed(end));
  };
  const track = (subscription: Promise<() => void>) => subscription.then(stop => {
    if (disposed) stop();
    else stops.push(stop);
  });

  const stream = {
    retained: false,
    exited: false,
    ready: Promise.resolve(),
    attach(load: (consumerId?: string) => Promise<TerminalSnapshot>) {
      attachment = load(consumerId);
      return attachment;
    },
    replay(snapshot: TerminalSnapshot, sink: (data: Uint8Array, done: () => void) => void, exit: () => void) {
      if (disposed) return;
      stream.retained = true;
      offset = snapshot.output_offset;
      write = sink;
      onExit = exit;
      stream.exited ||= !snapshot.live;
      // Register input and cache ownership before replay: queries can emit replies.
      sink(decodeBase64(snapshot.scrollback), () => parsed(snapshot.output_offset));
      pending.sort((a, b) => a.offset - b.offset).forEach(deliver);
      pending = [];
      if (stream.exited) exit();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      clearTimeout(ackTimer);
      // An attach already in flight may acquire credit after unmount. Release
      // only once it settles. The token protects a newer receiver from late
      // release/ACK messages belonging to this subscription.
      if (flow) {
        const release = (delay = 50): void => {
          void flow.release(consumerId!).catch(error => {
            console.warn("Terminal output release failed", error);
            setTimeout(() => release(Math.min(1000, delay * 2)), delay);
          });
        };
        if (attachment) void attachment.then(() => release(), () => release());
        else release();
      }
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
