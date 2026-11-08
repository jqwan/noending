import { clearReadResources } from "../../hooks/readResource";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import { mergeMessages, useCollaborationRoom } from "./useCollaborationRoom";
import type { CollaborationMessage, CollaborationRoomView } from "../../types";

const events = vi.hoisted(() => ({ onChange: null as null | ((event: { payload: unknown }) => void) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("../../api", () => ({ api: { collaborationRoomGet: vi.fn(), collaborationRoomMessages: vi.fn() } }));

const room = (task = "task", id = "room"): CollaborationRoomView => ({
  task_id: task, title: task,
  room: { id, revision: 1, selected_members: [], coordinator_session_id: null },
  run: null, members: [],
});
type MessagePage = Awaited<ReturnType<typeof api.collaborationRoomMessages>>;
const range = (from: number, through: number) => Array.from({ length: through - from + 1 }, (_, i) => message(from + i));

beforeEach(() => {
  clearReadResources();
  vi.resetAllMocks();
  events.onChange = null;
  vi.mocked(listen).mockImplementation(async (_name, callback) => {
    events.onChange = callback as (event: { payload: unknown }) => void;
    return () => { events.onChange = null; };
  });
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(room());
});
afterEach(cleanup);

function notify() {
  events.onChange!({ payload: { task_id: "task", room_id: "room", category: "message" } });
}

const message = (seq: number, patch: Partial<CollaborationMessage> = {}): CollaborationMessage => ({
  id: `m-${seq}`,
  room_id: "room",
  run_id: "run",
  seq,
  sender_session_id: "s1",
  body: `body ${seq}`,
  kind: "text",
  created_at: `2026-10-27T03:${String(seq).padStart(2, "0")}:00Z`,
  ...patch,
});

it("reads room metadata without history in task details and catches up when entering chat", async () => {
  vi.mocked(api.collaborationRoomMessages).mockResolvedValue({ items: [message(1)], next_cursor: null, latest_seq: 1 });
  const view = renderHook(({ chat }) => useCollaborationRoom("task", chat), { initialProps: { chat: false } });
  await waitFor(() => expect(view.result.current.roomLoading).toBe(false));
  expect(view.result.current.room?.room?.id).toBe("room");
  expect(api.collaborationRoomMessages).not.toHaveBeenCalled();
  view.rerender({ chat: true });
  await waitFor(() => expect(view.result.current.messages.map(m => m.seq)).toEqual([1]));
  view.rerender({ chat: false });
  vi.mocked(api.collaborationRoomMessages).mockClear();
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({ ...room(), title: "外部更新" });
  await act(async () => { notify(); });
  await waitFor(() => expect(view.result.current.room?.title).toBe("外部更新"));
  expect(api.collaborationRoomMessages).not.toHaveBeenCalled();
  vi.mocked(api.collaborationRoomMessages).mockResolvedValue({ items: [message(1), message(2)], next_cursor: null, latest_seq: 2 });
  view.rerender({ chat: true });
  await waitFor(() => expect(view.result.current.messages.map(m => m.seq)).toEqual([1, 2]));
});

it("appends a newer page to the single shared sequence without duplicating or reordering", () => {
  // 用户与成员的发言共用一条序列：合并只按 ID 去重、按 seq 排序。
  const merged = mergeMessages(
    [message(1, { sender_session_id: null }), message(2)],
    [message(2, { sender_session_id: null }), message(3)],
  );
  expect(merged.map(m => m.seq)).toEqual([1, 2, 3]);
});

it("preserves known history when merging a page beyond a gap", () => {
  const merged = mergeMessages([message(1), message(2)], [message(40), message(41)]);
  expect(merged.map(m => m.seq)).toEqual([1, 2, 40, 41]);
});

it("prepends an earlier page in sequence order", () => {
  const merged = mergeMessages([message(5), message(6)], [message(3), message(4)]);
  expect(merged.map(m => m.seq)).toEqual([3, 4, 5, 6]);
});

it("leaves an empty page alone instead of clearing what is already shown", () => {
  const merged = mergeMessages([message(1)], []);
  expect(merged.map(m => m.seq)).toEqual([1]);
});

it("fills a multi-page burst without losing already displayed history", async () => {
  let updated = false;
  vi.mocked(api.collaborationRoomMessages).mockImplementation(async (_room, _run, cursor) => {
    if (!updated) return { items: range(1, 100), next_cursor: null, latest_seq: 100 };
    if (cursor === "before-202") return { items: range(102, 201), next_cursor: "before-102", latest_seq: 301 };
    if (cursor === "before-102") return { items: range(2, 101), next_cursor: "before-2", latest_seq: 301 };
    if (cursor === "before-2") return { items: range(1, 1), next_cursor: null, latest_seq: 301 };
    return { items: range(202, 301), next_cursor: "before-202", latest_seq: 301 };
  });
  const { result } = renderHook(() => useCollaborationRoom("task"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.messages).toHaveLength(100);
  updated = true;
  await act(async () => { notify(); });
  await waitFor(() => expect(result.current.messages).toHaveLength(301));
  expect(result.current.messages[0].seq).toBe(1);
  expect(result.current.messages[149].seq).toBe(150);
  expect(result.current.messages[300].seq).toBe(301);
});

it("keeps older history and the new message when notification arrives during initial loading", async () => {
  let release!: (page: MessagePage) => void;
  const olderPage = new Promise<MessagePage>(resolve => { release = resolve; });
  let signalOlder!: () => void;
  const olderStarted = new Promise<void>(resolve => { signalOlder = resolve; });
  let updated = false;
  vi.mocked(api.collaborationRoomMessages).mockImplementation(async (_room, _run, cursor) => {
    if (cursor === "before-20") { signalOlder(); return olderPage; }
    if (cursor === "before-21") return { items: range(1, 20), next_cursor: null, latest_seq: 120 };
    return updated
      ? { items: range(21, 120), next_cursor: "before-21", latest_seq: 120 }
      : { items: range(20, 119), next_cursor: "before-20", latest_seq: 119 };
  });
  const { result } = renderHook(() => useCollaborationRoom("task"));
  await act(async () => { await olderStarted; });
  updated = true;
  await act(async () => { notify(); });
  await act(async () => {
    release({ items: range(1, 19), next_cursor: null, latest_seq: 119 });
    await olderPage;
  });
  await waitFor(() => expect(result.current.loading).toBe(false));
  await waitFor(() => expect(result.current.messages).toHaveLength(120));
  expect(result.current.messages[0].seq).toBe(1);
  expect(result.current.messages[119].seq).toBe(120);
});

it("does not let an old task's delayed history overwrite the new task", async () => {
  let release!: (page: MessagePage) => void;
  const oldPage = new Promise<MessagePage>(resolve => { release = resolve; });
  let signalOld!: () => void;
  const oldStarted = new Promise<void>(resolve => { signalOld = resolve; });
  vi.mocked(api.collaborationRoomGet).mockImplementation(async task => room(task, task));
  vi.mocked(api.collaborationRoomMessages).mockImplementation(async id => {
    if (id === "old") { signalOld(); return oldPage; }
    return { items: [message(10, { room_id: "new", body: "new task" })], next_cursor: null, latest_seq: 10 };
  });
  const { result, rerender } = renderHook(({ task }) => useCollaborationRoom(task), { initialProps: { task: "old" } });
  await act(async () => { await oldStarted; });
  rerender({ task: "new" });
  await waitFor(() => expect(result.current.messages[0]?.body).toBe("new task"));
  await act(async () => {
    release({ items: [message(1, { room_id: "old", body: "old task" })], next_cursor: null, latest_seq: 1 });
    await oldPage;
  });
  expect(result.current.messages.map(item => item.body)).toEqual(["new task"]);
});

it("reads past one hundred history pages instead of silently truncating older messages", async () => {
  const total = 10_001;
  vi.mocked(api.collaborationRoomMessages).mockImplementation(async (_room, _run, cursor) => {
    const last = cursor == null ? total : Number(cursor.slice("before-".length)) - 1;
    const first = Math.max(1, last - 99);
    return { items: range(first, last), next_cursor: first > 1 ? `before-${first}` : null, latest_seq: total };
  });
  const { result } = renderHook(() => useCollaborationRoom("task"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.messages).toHaveLength(total);
  expect(result.current.messages[0].seq).toBe(1);
  expect(result.current.messages[total - 1].seq).toBe(total);
});

it("reports a repeated history cursor without presenting the partial result as complete", async () => {
  vi.mocked(api.collaborationRoomMessages).mockResolvedValue({
    items: [message(2)], next_cursor: "repeated-cursor", latest_seq: 2,
  });
  const { result } = renderHook(() => useCollaborationRoom("task"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.error).toContain("分页游标未推进");
  expect(result.current.messages).toEqual([]);
});
