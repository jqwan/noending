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
  body, kind: "text", created_at: `2026-10-27T03:0${seq}:00Z`,
});

const member = (patch: Partial<CollaborationMemberState> & { session_id: string }): CollaborationMemberState => ({
  wake_method: "terminal", wake_status: "opened", connection_state: "connected", phase: "listening",
  last_heartbeat_at: "2026-10-27T03:59:00Z", last_read_at: "2026-10-27T03:58:00Z", last_read_seq: 1,
  awaiting_since: null, unread_count: 0, error: null, ...patch,
});

const noRoom: RoomView = { task_id: "task-1", title: "跨会话聊天室开发", room: null, run: null, members: [] };
const activeRun: RoomView = {
  task_id: "task-1",
  title: "跨会话聊天室开发",
  room: {
    id: "room-1",
    revision: 3,
    selected_members: [
      { session_id: "s1", wake_method: "terminal" },
      { session_id: "s2", wake_method: "desktop" },
    ],
    coordinator_session_id: "s1",
  },
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

it("reuses the saved wake method when a stopped room is started again", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(stoppedRun);
  vi.mocked(api.collaborationRoomStart).mockResolvedValue(activeRun);
  mount();
  const start = await screen.findByRole("button", { name: "开启协作" }) as HTMLButtonElement;
  await waitFor(() => expect(start.disabled).toBe(false));
  fireEvent.click(start);

  await waitFor(() => expect(api.collaborationRoomStart).toHaveBeenCalledWith(
    "task-1",
    // 配置里保存的唤醒方式（s2 是桌面）被沿用，不在重开时重新推断。
    [{ session_id: "s1", wake_method: "terminal" }, { session_id: "s2", wake_method: "desktop" }],
    3,
    expect.any(String),
    "s1",
    null,
  ));
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

  // 两版契约的心跳类命名都只刷成员状态：不重载消息历史，也不制造重复条目。
  const set = handlers.get("collaboration-changed");
  expect(set?.size).toBeGreaterThan(0);
  for (const category of ["member", "heartbeat", "ack", "wake"]) {
    for (const handler of set ?? []) {
      handler({ payload: { task_id: "task-1", room_id: "room-1", run_id: "run-1", category } });
    }
  }

  await waitFor(() => expect(
    within(screen.getByRole("listitem", { name: "后端会话" })).getByText("正在监听"),
  ).toBeTruthy());
  expect(screen.queryByText("等待加入")).toBeNull();
  expect(screen.getAllByRole("listitem").length).toBe(before);
  expect(screen.getByText("先对齐契约")).toBeTruthy();
});
