import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import CollaborationRoomView from "./CollaborationRoomView";
import type {
  CollaborationCandidate,
  CollaborationMemberState,
  CollaborationMessage,
  CollaborationRoomView as RoomView,
  SessionListItem,
} from "../../types";

const { handlers } = vi.hoisted(() => ({ handlers: new Map<string, Set<(event: { payload: unknown }) => void>>() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("../../api", () => ({ api: {
  collaborationRoomGet: vi.fn(),
  collaborationRoomMessages: vi.fn(),
  collaborationCandidates: vi.fn(),
  collaborationRoomStart: vi.fn(),
  collaborationRoomStop: vi.fn(),
  collaborationRoomSend: vi.fn(),
  collaborationRoomCoordinator: vi.fn(),
  collaborationMemberWake: vi.fn(),
} }));

const sessions = [
  { id: "s1", title: "后端会话" },
  { id: "s2", title: "前端会话" },
  { id: "s3", title: "归档会话" },
] as SessionListItem[];

const candidates: CollaborationCandidate[] = [
  { session_id: "s1", title: "后端会话", agent: "codex", wake_modes: ["terminal"], unavailable_reason: null },
  { session_id: "s2", title: "前端会话", agent: "claude_code", wake_modes: ["terminal", "desktop"], unavailable_reason: null },
  { session_id: "s3", title: "归档会话", agent: "pi", wake_modes: ["desktop"], unavailable_reason: "会话已归档" },
];

const message = (seq: number, body: string, sender: string | null = "s1"): CollaborationMessage => ({
  id: `m-${seq}`, room_id: "room-1", run_id: "run-1", seq, sender_session_id: sender,
  body, kind: "text", created_at: `2026-10-27T03:${String(seq).padStart(2, "0")}:00Z`,
});

const member = (patch: Partial<CollaborationMemberState> & { session_id: string }): CollaborationMemberState => ({
  wake_method: "terminal", wake_status: "opened", connection_state: "connected", phase: "listening",
  last_heartbeat_at: "2026-10-27T03:59:00Z", last_read_at: "2026-10-27T03:58:00Z", last_read_seq: 1,
  awaiting_since: null, unread_count: 0, error: null, ...patch,
});

const roomConfig = {
  id: "room-1",
  revision: 3,
  selected_members: [
    { session_id: "s1", wake_method: "terminal" as const },
    { session_id: "s2", wake_method: "desktop" as const },
  ],
  coordinator_session_id: "s1" as string | null,
};

const noRoom: RoomView = { task_id: "task-1", title: "跨会话聊天室开发", room: null, run: null, members: [] };
const activeRun: RoomView = {
  task_id: "task-1",
  title: "跨会话聊天室开发",
  room: roomConfig,
  run: { id: "run-1", status: "active", started_at: "2026-10-27T03:50:00Z", stopped_at: null },
  members: [member({ session_id: "s1" }), member({ session_id: "s2", phase: "processing" })],
};
const stoppedRun: RoomView = {
  ...activeRun,
  run: { ...activeRun.run!, status: "stopped", stopped_at: "2026-10-27T03:58:00Z" },
};

function mount() {
  return render(<CollaborationRoomView taskId="task-1" taskTitle="跨会话聊天室开发" sessions={sessions} />);
}

function emit(payload: Record<string, unknown>) {
  for (const handler of handlers.get("collaboration-changed") ?? []) handler({ payload });
}

beforeEach(() => {
  vi.resetAllMocks();
  handlers.clear();
  vi.mocked(listen).mockImplementation(async (name, callback) => {
    const fn = callback as unknown as (event: { payload: unknown }) => void;
    const set = handlers.get(name) ?? new Set();
    set.add(fn);
    handlers.set(name, set);
    return () => { set.delete(fn); };
  });
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(noRoom);
  vi.mocked(api.collaborationRoomMessages).mockResolvedValue({ items: [message(1, "先对齐契约")], next_cursor: null, latest_seq: 1 });
  vi.mocked(api.collaborationCandidates).mockResolvedValue({ items: candidates, next_cursor: null });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it("keeps 开启协作 disabled until two session members are chosen and shows why a session cannot join", async () => {
  mount();
  await screen.findByRole("checkbox", { name: "后端会话" });
  const start = screen.getByRole("button", { name: "开启协作" }) as HTMLButtonElement;
  expect(start.disabled).toBe(true);

  // 不可用会话照常列出原因，而不是悄悄从候选里消失。
  const archived = screen.getByRole("checkbox", { name: "归档会话" }) as HTMLInputElement;
  expect(archived.disabled).toBe(true);
  expect(screen.getByText("会话已归档")).toBeTruthy();

  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  expect(screen.getByText("还需要再选 1 个会话才能开启协作。")).toBeTruthy();
  expect((screen.getByRole("button", { name: "开启协作" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  await waitFor(() => expect((screen.getByRole("button", { name: "开启协作" }) as HTMLButtonElement).disabled).toBe(false));
});

it("writes the first message together with starting, and shows the run and member states after it", async () => {
  vi.mocked(api.collaborationRoomStart).mockResolvedValue(activeRun);
  mount();
  await screen.findByRole("checkbox", { name: "后端会话" });
  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  fireEvent.click(screen.getByRole("radio", { name: "后端会话" }));
  fireEvent.change(screen.getByRole("textbox", { name: "首条消息（随开启一起写入）" }), { target: { value: "先各自核对契约" } });
  fireEvent.click(screen.getByRole("button", { name: "发送并开启" }));

  await screen.findByText("协作中");
  expect(api.collaborationRoomStart).toHaveBeenCalledWith(
    "task-1",
    [{ session_id: "s1", wake_method: "terminal" }, { session_id: "s2", wake_method: "terminal" }],
    0,
    expect.any(String),
    "s1",
    "先各自核对契约",
  );
  const members = screen.getByRole("list", { name: "成员状态" });
  expect(within(members).getByRole("listitem", { name: "后端会话" })).toBeTruthy();
  expect(within(members).getByText("处理中")).toBeTruthy();
});

it("changes and clears the coordinator while the run is active", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomCoordinator)
    .mockResolvedValueOnce({ ...activeRun, room: { ...roomConfig, revision: 4, coordinator_session_id: "s2" } })
    .mockResolvedValueOnce({ ...activeRun, room: { ...roomConfig, revision: 5, coordinator_session_id: null } });
  mount();

  const backend = await screen.findByRole("radio", { name: "后端会话" }) as HTMLInputElement;
  expect(backend.checked).toBe(true);

  fireEvent.click(screen.getByRole("radio", { name: "前端会话" }));
  await waitFor(() => expect((screen.getByRole("radio", { name: "前端会话" }) as HTMLInputElement).checked).toBe(true));
  // 更换负责人带当前配置版本，冲突时由后端拒绝。
  expect(api.collaborationRoomCoordinator).toHaveBeenLastCalledWith("room-1", "run-1", "s2", 3, expect.any(String));

  fireEvent.click(screen.getByRole("radio", { name: "不由成员负责，由我协调" }));
  await waitFor(() => expect((screen.getByRole("radio", { name: "不由成员负责，由我协调" }) as HTMLInputElement).checked).toBe(true));
  expect(api.collaborationRoomCoordinator).toHaveBeenLastCalledWith("room-1", "run-1", null, 4, expect.any(String));
});

it("lets the user drop every member and clear the saved coordinator after a stop", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(stoppedRun);
  vi.mocked(api.collaborationRoomStart).mockResolvedValue(activeRun);
  mount();

  // 停止后沿用保存的配置：两人都选中，负责人仍是 s1。
  expect((await screen.findByRole("checkbox", { name: "后端会话" }) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByRole("checkbox", { name: "前端会话" }) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByRole("radio", { name: "后端会话" }) as HTMLInputElement).checked).toBe(true);

  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  // 取消全部成员后不能被配置悄悄回填。
  await waitFor(() => expect((screen.getByRole("checkbox", { name: "后端会话" }) as HTMLInputElement).checked).toBe(false));
  expect((screen.getByRole("checkbox", { name: "前端会话" }) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("button", { name: "开启协作" }) as HTMLButtonElement).disabled).toBe(true);

  // 重新选人并清除原负责人，再开启时负责人是 null 而不是被回填成 s1。
  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  fireEvent.click(screen.getByRole("radio", { name: "不由成员负责，由我协调" }));
  await waitFor(() => expect((screen.getByRole("radio", { name: "不由成员负责，由我协调" }) as HTMLInputElement).checked).toBe(true));
  fireEvent.click(screen.getByRole("button", { name: "开启协作" }));

  await waitFor(() => expect(api.collaborationRoomStart).toHaveBeenCalledWith(
    "task-1",
    [{ session_id: "s1", wake_method: "terminal" }, { session_id: "s2", wake_method: "terminal" }],
    3,
    expect.any(String),
    null,
    null,
  ));
});

it("keeps messages readable after stopping and reports that this run ended", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomStop).mockResolvedValue(stoppedRun);
  mount();
  await screen.findByRole("button", { name: "停止协作" });
  fireEvent.click(screen.getByRole("button", { name: "停止协作" }));

  await screen.findByText(/上次协作已停止/);
  expect(screen.getByRole("button", { name: "开启协作" })).toBeTruthy();
  // 正文仍可查阅：停止不删消息，也不把成员状态写成仍在工作。
  expect(screen.getByText("先对齐契约")).toBeTruthy();
  expect(screen.queryByRole("list", { name: "成员状态" })).toBeNull();
});

it("says the run has stopped instead of reporting a generic failure when the old run refuses a message", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomSend).mockRejectedValue(new Error("run_inactive: 本次运行已结束"));
  mount();
  await screen.findByRole("button", { name: "停止协作" });
  fireEvent.change(screen.getByRole("textbox", { name: "在协作室说一句" }), { target: { value: "还在吗" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));

  await screen.findByText(/本次协作已停止/);
  // 草稿保留，用户可以复制到新运行，而不是被静默吞掉。
  expect((screen.getByRole("textbox", { name: "在协作室说一句" }) as HTMLTextAreaElement).value).toBe("还在吗");
});

it("reuses the same request id when a failed send is retried unchanged and starts a new one when the text changes", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomSend)
    .mockRejectedValueOnce(new Error("timeout"))
    .mockResolvedValue(message(3, "还在吗"));
  mount();
  await screen.findByRole("button", { name: "停止协作" });
  const box = screen.getByRole("textbox", { name: "在协作室说一句" });
  fireEvent.change(box, { target: { value: "还在吗" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));

  await screen.findByText(/发送失败/);
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(api.collaborationRoomSend).toHaveBeenCalledTimes(2));
  const calls = vi.mocked(api.collaborationRoomSend).mock.calls;
  // 后端可能已经落库、只是首次响应超时：重试必须重放同一个 request_id。
  expect(calls[0][3]).toBe(calls[1][3]);

  fireEvent.change(box, { target: { value: "还在吗？" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(api.collaborationRoomSend).toHaveBeenCalledTimes(3));
  // 正文变了就是新操作，不能复用已被消费的 request_id。
  expect(vi.mocked(api.collaborationRoomSend).mock.calls[2][3]).not.toBe(calls[0][3]);
});

it("drops the stale earlier cursor when a newer page leaves a hole, then fills the gap without loss or duplication", async () => {
  const first = [message(1, "一"), message(2, "二")];
  const later = Array.from({ length: 30 }, (_, i) => message(40 + i, `m${40 + i}`));
  const gap = Array.from({ length: 37 }, (_, i) => message(3 + i, `m${3 + i}`));
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomMessages)
    .mockResolvedValueOnce({ items: first, next_cursor: "cursor-before-1", latest_seq: 2 })
    .mockResolvedValueOnce({ items: later, next_cursor: "cursor-39", latest_seq: 69 })
    .mockResolvedValueOnce({ items: gap, next_cursor: null, latest_seq: 69 });
  mount();
  await screen.findByText("一");

  emit({ task_id: "task-1", room_id: "room-1", run_id: "run-1", category: "message" });
  await screen.findByText("m40");
  // 中间空档：旧页被丢弃，旧的向前游标也必须一起作废。
  expect(screen.queryByText("一")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "加载更早的消息" }));
  await screen.findByText("m3");
  // 请求用的是新页给的游标，否则 3..39 会被永久跳过。
  expect(vi.mocked(api.collaborationRoomMessages).mock.calls[2][2]).toBe("cursor-39");

  const list = screen.getByRole("list", { name: "协作消息" });
  expect(within(list).getAllByRole("listitem")).toHaveLength(67);
  expect(within(list).getAllByText("#3")).toHaveLength(1);
  expect(within(list).getAllByText("#69")).toHaveLength(1);
});

it("replays an unconfirmed start instead of sending the first message a second time", async () => {
  vi.mocked(api.collaborationRoomStart)
    .mockRejectedValueOnce(new Error("timeout"))
    .mockResolvedValueOnce(activeRun);
  // 后端其实已经落库：刷新后房间是运行中，只是首次响应丢了。
  vi.mocked(api.collaborationRoomGet)
    .mockResolvedValueOnce(noRoom)
    .mockResolvedValue(activeRun);
  mount();
  await screen.findByRole("checkbox", { name: "后端会话" });
  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  fireEvent.click(screen.getByRole("radio", { name: "后端会话" }));
  fireEvent.change(screen.getByRole("textbox", { name: "首条消息（随开启一起写入）" }), { target: { value: "先各自核对契约" } });
  fireEvent.click(screen.getByRole("button", { name: "发送并开启" }));

  await screen.findByText(/开启未确认/);
  await screen.findByRole("button", { name: "停止协作" });
  // 草稿还在：用户看不出这是一条已经写进房间的消息。
  expect((screen.getByRole("textbox", { name: "在协作室说一句" }) as HTMLTextAreaElement).value).toBe("先各自核对契约");

  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(api.collaborationRoomStart).toHaveBeenCalledTimes(2));
  const calls = vi.mocked(api.collaborationRoomStart).mock.calls;
  // 原参数、原 request_id 重放，而不是按刷新后的配置另起一次开启。
  expect(calls[1][3]).toBe(calls[0][3]);
  expect(calls[1][1]).toEqual(calls[0][1]);
  // 关键：没有再发一条正文，房间里只有一份首条消息。
  expect(api.collaborationRoomSend).not.toHaveBeenCalled();
});

it("keeps an unconfirmed send request id when another member wake happens in between", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({
    ...activeRun,
    members: [
      member({ session_id: "s1" }),
      member({ session_id: "s2", wake_status: "failed", connection_state: "pending", phase: null }),
    ],
  });
  vi.mocked(api.collaborationRoomSend)
    .mockRejectedValueOnce(new Error("timeout"))
    .mockResolvedValue(message(3, "在吗"));
  vi.mocked(api.collaborationMemberWake).mockResolvedValue(member({ session_id: "s2" }));
  mount();
  await screen.findByText("唤醒失败");

  fireEvent.change(screen.getByRole("textbox", { name: "在协作室说一句" }), { target: { value: "在吗" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await screen.findByText(/发送失败/);

  // 期间发生另一次写操作（唤醒），不能顶掉这次未确认发信的 request_id。
  fireEvent.click(screen.getByRole("button", { name: "重试打开" }));
  await waitFor(() => expect(api.collaborationMemberWake).toHaveBeenCalled());

  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(api.collaborationRoomSend).toHaveBeenCalledTimes(2));
  const calls = vi.mocked(api.collaborationRoomSend).mock.calls;
  expect(calls[1][3]).toBe(calls[0][3]);
});

it("shows start notices as a label instead of their raw coordinates", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomMessages).mockResolvedValue({
    items: [
      { ...message(1, ""), kind: "start", body: '{"task_id":"task-1","room_id":"room-1","run_id":"run-1"}', sender_session_id: null },
      message(2, "开始吧"),
    ],
    next_cursor: null,
    latest_seq: 2,
  });
  mount();
  await screen.findByText("开始吧");
  expect(screen.getByText("开始协作")).toBeTruthy();
  expect(screen.queryByText(/\{"task_id"/)).toBeNull();
});

it("marks a listening member unresponsive while leaving a processing member alone", async () => {
  const stale = new Date(Date.now() - 3 * 60 * 1000).toISOString();
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({
    ...activeRun,
    members: [
      member({ session_id: "s1", awaiting_since: stale }),
      member({ session_id: "s2", phase: "processing", awaiting_since: stale }),
    ],
  });
  mount();
  const listening = within(await screen.findByRole("listitem", { name: "后端会话" }));
  expect(listening.getByText("暂未响应")).toBeTruthy();

  // 处理中不套 2 分钟门槛：只在处理时长上说话。
  const processing = within(screen.getByRole("listitem", { name: "前端会话" }));
  expect(processing.getByText("处理中")).toBeTruthy();
  expect(processing.queryByText("暂未响应")).toBeNull();
});

it("retries opening one failed member without restarting the run for everyone", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({
    ...activeRun,
    members: [
      member({ session_id: "s1" }),
      member({ session_id: "s2", wake_status: "failed", connection_state: "pending", phase: null, error: "应用未响应" }),
    ],
  });
  vi.mocked(api.collaborationMemberWake).mockResolvedValue(member({ session_id: "s2" }));
  mount();
  await screen.findByText("唤醒失败");
  fireEvent.click(screen.getByRole("button", { name: "重试打开" }));

  await waitFor(() => expect(screen.queryByText("唤醒失败")).toBeNull());
  const retried = within(screen.getByRole("listitem", { name: "前端会话" }));
  expect(retried.getByText("正在监听")).toBeTruthy();
  expect(screen.queryByText("应用未响应")).toBeNull();
  // 只重试这一个成员，不重新开启整次协作。
  expect(api.collaborationRoomStart).not.toHaveBeenCalled();
  expect(api.collaborationMemberWake).toHaveBeenCalledWith("room-1", "run-1", "s2", expect.any(String));
});

it("refreshes only member state on heartbeat-class changes and keeps the message list single", async () => {
  vi.mocked(api.collaborationRoomGet)
    .mockResolvedValueOnce({ ...activeRun, members: [member({ session_id: "s1", connection_state: "pending", phase: null }), member({ session_id: "s2" })] })
    .mockResolvedValue(activeRun);
  mount();
  await screen.findByText("等待加入");
  const before = screen.getAllByRole("listitem").length;

  // member（心跳）与 room（配置）只刷房间，不重载消息历史、不制造重复条目。
  expect(handlers.get("collaboration-changed")?.size).toBeGreaterThan(0);
  emit({ task_id: "task-1", room_id: "room-1", run_id: "run-1", category: "member" });
  emit({ task_id: "task-1", room_id: "room-1", run_id: "run-1", category: "room" });

  await waitFor(() => expect(
    within(screen.getByRole("listitem", { name: "后端会话" })).getByText("正在监听"),
  ).toBeTruthy());
  expect(screen.queryByText("等待加入")).toBeNull();
  expect(screen.getAllByRole("listitem").length).toBe(before);
  expect(screen.getByText("先对齐契约")).toBeTruthy();
});
