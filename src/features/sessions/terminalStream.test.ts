import { waitFor } from "@testing-library/react";
import { afterAll, afterEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import type { TerminalOutput, TerminalSnapshot } from "../../types";
import { subscribeTerminalStream, type TerminalStream } from "./terminalStream";

const { handlers } = vi.hoisted(() => ({ handlers: new Map<string, (event: unknown) => void>() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (event: unknown) => void) => {
    handlers.set(event, handler);
    return Promise.resolve(() => handlers.delete(event));
  }),
}));
// Use the real xterm parser without opening a renderer.
const canvas = vi.hoisted(() => vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null));
afterAll(() => canvas.mockRestore());
const streams: TerminalStream[] = [];
const terminals: Terminal[] = [];
afterEach(() => {
  streams.splice(0).forEach(stream => stream.dispose());
  terminals.splice(0).forEach(term => term.dispose());
  handlers.clear();
  vi.clearAllMocks();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function subscribe() {
  const stream = subscribeTerminalStream("t-1");
  streams.push(stream);
  await stream.ready;
  return stream;
}
function output(offset: number, data: string) {
  const payload: TerminalOutput = { offset, data: btoa(data) };
  handlers.get("terminal-output://t-1")!({ payload });
}
function snapshot(scrollback: string): TerminalSnapshot {
  return {
    terminal_id: "t-1", session_id: null, initial_workstream_ids: [], identity_revision: 0,
    agent: "codex", cwd: null, created_at: "", live: true, exit_code: null, session_title: null,
    scrollback: btoa(scrollback), output_offset: scrollback.length, cols: 80, rows: 24,
  };
}

it("replies to ConPTY's cursor query received after the snapshot boundary but before xterm exists", async () => {
  const stream = await subscribe();
  const initial = snapshot("");
  output(0, "\x1b[6n");
  const term = new Terminal({ cols: 80, rows: 24 });
  terminals.push(term);
  const replies: string[] = [];
  term.onData(data => replies.push(data));
  stream.replay(initial, bytes => term.write(bytes), () => {});
  await waitFor(() => expect(replies).toEqual(["\x1b[1;1R"]));
});

it("replays overlapping snapshot/events exactly once at raw-byte boundaries", async () => {
  const stream = await subscribe();
  // The snapshot cuts through a UTF-8 character; do not decode each chunk to text.
  const raw = String.fromCharCode(0xe4, 0xbd, 0xa0) + "!";
  output(0, raw);
  output(4, "ready");
  const chunks: Uint8Array[] = [];
  stream.replay(snapshot(raw.slice(0, 2)), bytes => chunks.push(bytes), () => {});
  // Late delivery of bytes already covered by the snapshot/events is harmless.
  output(0, raw);
  output(9, "!");
  const bytes = Uint8Array.from(chunks.flatMap(chunk => Array.from(chunk)));
  expect(new TextDecoder().decode(bytes)).toBe("你!ready!");
});

it("acknowledges only parsed raw bytes, including replay and overlap, and cancels on dispose", async () => {
  const acknowledge = vi.fn().mockResolvedValue(undefined);
  const stream = subscribeTerminalStream("t-1", { acknowledge, release: vi.fn().mockResolvedValue(undefined) });
  streams.push(stream);
  await stream.ready;
  const completed: (() => void)[] = [];
  output(1, "bc");
  stream.replay(snapshot("ab"), (_, done) => completed.push(done), () => {});
  expect(acknowledge).not.toHaveBeenCalled();
  completed[0]();
  await waitFor(() => expect(acknowledge).toHaveBeenLastCalledWith(2, expect.any(String)));
  completed[1]();
  await waitFor(() => expect(acknowledge).toHaveBeenLastCalledWith(3, expect.any(String)));
  output(0, "abc");
  expect(completed).toHaveLength(2);
  output(3, "d");
  completed[2]();
  stream.dispose();
  await new Promise(resolve => setTimeout(resolve, 5));
  expect(acknowledge.mock.calls.map(call => call[0])).toEqual([2, 3]);
});


it("releases a cancelled attachment only after acquisition settles, using its own consumer", async () => {
  vi.useFakeTimers();
  vi.spyOn(console, "warn").mockImplementation(() => {});
  const release = vi.fn().mockRejectedValueOnce(new Error("temporary release failure")).mockResolvedValue(undefined);
  const stream = subscribeTerminalStream("t-1", { acknowledge: vi.fn(), release });
  streams.push(stream);
  await stream.ready;
  let finish!: (value: TerminalSnapshot) => void;
  let owner: string | undefined;
  const attached = stream.attach(consumer => { owner = consumer; return new Promise(resolve => { finish = resolve; }); });
  stream.dispose();
  expect(release).not.toHaveBeenCalled();
  finish(snapshot("pending"));
  await attached;
  expect(release.mock.calls).toEqual([[owner]]);
  stream.dispose();
  expect(release).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(100);
  expect(release.mock.calls).toEqual([[owner], [owner]]);
});

it("retries failed parser acknowledgements even when no further output arrives, and stops after disposal", async () => {
  vi.useFakeTimers();
  vi.spyOn(console, "warn").mockImplementation(() => {});
  const acknowledge = vi.fn().mockRejectedValueOnce(new Error("temporary IPC failure")).mockResolvedValue(undefined);
  const stream = subscribeTerminalStream("t-1", { acknowledge, release: vi.fn().mockResolvedValue(undefined) });
  streams.push(stream);
  await stream.ready;
  stream.replay(snapshot("tail"), (_, done) => done(), () => {});
  await vi.advanceTimersByTimeAsync(100);
  expect(acknowledge.mock.calls.map(call => call[0])).toEqual([4, 4]);
  acknowledge.mockRejectedValue(new Error("offline"));
  output(4, "next");
  await vi.advanceTimersByTimeAsync(0);
  stream.dispose();
  const count = acknowledge.mock.calls.length;
  await vi.advanceTimersByTimeAsync(2000);
  expect(acknowledge).toHaveBeenCalledTimes(count);
});
