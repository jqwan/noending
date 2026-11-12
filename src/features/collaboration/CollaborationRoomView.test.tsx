import { clearReadResources } from "../../hooks/readResource";
import { DetailPanelProvider } from "../../components/DetailLayout";
import { copyToClipboard } from "../../components/common";
import PageHeader from "../../layout/PageHeader";
import { viewState } from "../../hooks/useViewState";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import CollaborationRoomView from "./CollaborationRoomView";
import RoomMessages from "./RoomMessages";
import type {
  CollaborationCandidate,
  CollaborationMemberState,
  CollaborationMessage,
  CollaborationRoomView as RoomView,
  SessionListItem,
} from "../../types";

const { handlers } = vi.hoisted(() => ({ handlers: new Map<string, Set<(event: { payload: unknown }) => void>>() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("../../components/common", async importOriginal => ({
  ...await importOriginal<typeof import("../../components/common")>(),
  copyToClipboard: vi.fn(),
}));
vi.mock("../../api", () => ({ api: {
  getOperationStatus: vi.fn(),
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

/** 心跳窗口只有 30 秒，测试数据必须相对当前时间，写死的时间戳会随日期腐坏。 */
const ago = (seconds: number) => new Date(Date.now() - seconds * 1000).toISOString();

const member = (patch: Partial<CollaborationMemberState> & { session_id: string }): CollaborationMemberState => ({
  wake_method: "terminal", wake_status: "opened", connection_state: "connected", phase: "listening",
  phase_since: null,
  last_heartbeat_at: ago(5), last_read_at: ago(60), last_read_seq: 1,
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
  return render(<DetailPanelProvider kind="collaboration"><PageHeader title="协作室" /><CollaborationRoomView taskId="task-1" taskTitle="跨会话聊天室开发" sessions={sessions} navigate={vi.fn()} /></DetailPanelProvider>);
}

function chooseCoordinator(name: string) {
  fireEvent.click(screen.getByRole("button", { name: /^负责人：/ }));
  fireEvent.click(within(screen.getByRole("listbox", { name: "负责人" })).getByRole("option", { name }));
}

function emit(payload: Record<string, unknown>) {
  for (const handler of handlers.get("collaboration-changed") ?? []) handler({ payload });
}

beforeEach(() => {
  clearReadResources();
  localStorage.clear(); viewState.clear();
  vi.resetAllMocks();
  handlers.clear();
  vi.mocked(api.getOperationStatus).mockResolvedValue([]);
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
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

it("sends with Enter but lets composition and Shift+Enter stay in the composer", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomSend).mockResolvedValue(message(2, "准备开始", null));
  mount();
  const box = await screen.findByRole("textbox", { name: "在协作室说一句" });
  expect(await within(screen.getByRole("form", { name: "发送协作消息" })).findByText("1 条消息")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "本次协作" })).queryByText("1 条消息")).toBeNull();
  fireEvent.change(box, { target: { value: "准备开始" } });
  fireEvent.keyDown(box, { key: "Enter", isComposing: true });
  fireEvent.keyDown(box, { key: "Enter", keyCode: 229 });
  fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
  expect(api.collaborationRoomSend).not.toHaveBeenCalled();
  expect((box as HTMLTextAreaElement).value).toBe("准备开始");
  fireEvent.keyDown(box, { key: "Enter" });
  await waitFor(() => expect(api.collaborationRoomSend).toHaveBeenCalledWith(
    "room-1", "run-1", "准备开始", expect.any(String),
  ));
  await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""));
});

it("shows members independently of message history and keeps them visible during a return visit", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomMessages).mockImplementation(() => new Promise(() => {}));
  const first = mount();
  const members = within(screen.getByRole("region", { name: "关联会话" }));
  await members.findByRole("listitem", { name: "后端会话" });
  expect(members.getByRole("listitem", { name: "前端会话" })).toBeTruthy();
  first.unmount();
  vi.mocked(api.collaborationRoomGet).mockImplementation(() => new Promise(() => {}));
  mount();
  const restored = within(screen.getByRole("region", { name: "关联会话" }));
  expect(restored.getByRole("listitem", { name: "后端会话" })).toBeTruthy();
  expect(restored.getByRole("listitem", { name: "前端会话" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "关联会话（3）" })).toBeTruthy();
});

it("restores stopped-room candidates immediately and retains them if revalidation fails", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(stoppedRun);
  const first = mount();
  await screen.findByRole("checkbox", { name: "后端会话" });
  first.unmount();
  let fail!: (reason: Error) => void;
  vi.mocked(api.collaborationCandidates).mockImplementation(() => new Promise((_, reject) => { fail = reject; }));
  mount();
  expect(screen.getByRole("checkbox", { name: "后端会话" })).toBeTruthy();
  expect(screen.queryByText("正在读取候选会话…")).toBeNull();
  await waitFor(() => expect(fail).toBeTypeOf("function"));
  await act(async () => { fail(new Error("候选读取失败")); });
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Error: 候选读取失败");
  expect(screen.getByRole("checkbox", { name: "后端会话" })).toBeTruthy();
});

it("keeps chosen members when their settings are hidden and restored", async () => {
  mount();
  const candidate = await screen.findByRole("checkbox", { name: "后端会话" });
  expect(screen.queryByRole("group", { name: "后端会话 的唤醒方式" })).toBeNull();
  fireEvent.click(candidate);
  expect(screen.getByRole("group", { name: "后端会话 的唤醒方式" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
  expect(screen.queryByRole("checkbox", { name: "后端会话" })).toBeNull();
  expect(screen.getByRole("textbox", { name: "首条消息（随开启一起写入）" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "展开详情栏" }));
  expect((screen.getByRole("checkbox", { name: "后端会话" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  expect(screen.queryByRole("group", { name: "后端会话 的唤醒方式" })).toBeNull();
});

it("leaves the reader on older messages until they choose to follow new ones", () => {
  const props = { sessions, sending: false, draft: "", onDraftChange: vi.fn(), onSend: vi.fn(), sendLabel: "发送", placeholder: "消息" };
  const view = render(<RoomMessages {...props} messages={[message(1, "旧消息")]} />);
  const history = screen.getByRole("region", { name: "协作消息记录" });
  Object.defineProperties(history, { scrollHeight: { configurable: true, value: 900 }, clientHeight: { configurable: true, value: 300 } });
  history.scrollTop = 0;
  fireEvent.scroll(history);
  view.rerender(<RoomMessages {...props} messages={[message(1, "旧消息"), message(2, "新消息")]} />);
  expect(screen.getByText("新消息")).toBeTruthy();
  expect(history.scrollTop).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "有新消息 · 回到最新消息" }));
  expect(history.scrollTop).toBe(900);
  expect(screen.queryByRole("button", { name: "有新消息 · 回到最新消息" })).toBeNull();
  Object.defineProperty(history, "scrollHeight", { value: 1100 });
  view.rerender(<RoomMessages {...props} messages={[message(1, "旧消息"), message(2, "新消息"), message(3, "后续消息")]} />);
  expect(history.scrollTop).toBe(1100);
});

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
  const wakeModes = within(screen.getByRole("group", { name: "前端会话 的唤醒方式" }));
  fireEvent.click(wakeModes.getByRole("button", { name: "桌面应用" }));
  expect(wakeModes.getByRole("button", { name: "桌面应用" }).getAttribute("aria-pressed")).toBe("true");
  expect(wakeModes.getByRole("button", { name: "内嵌终端" }).getAttribute("aria-pressed")).toBe("false");
  chooseCoordinator("后端会话");
  fireEvent.change(screen.getByRole("textbox", { name: "首条消息（随开启一起写入）" }), { target: { value: "先各自核对契约" } });
  fireEvent.click(screen.getByRole("button", { name: "发送并开启" }));

  await screen.findByText("协作中");
  expect(api.collaborationRoomStart).toHaveBeenCalledWith(
    "task-1",
    [{ session_id: "s1", wake_method: "terminal" }, { session_id: "s2", wake_method: "desktop" }],
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

  await screen.findByRole("button", { name: "负责人：后端会话" });

  chooseCoordinator("前端会话");
  await screen.findByRole("button", { name: "负责人：前端会话" });
  expect(within(screen.getByRole("listitem", { name: "前端会话" })).getByRole("img", { name: "负责人" })).toBeTruthy();
  // 更换负责人带当前配置版本，冲突时由后端拒绝。
  expect(api.collaborationRoomCoordinator).toHaveBeenLastCalledWith("room-1", "run-1", "s2", 3, expect.any(String));

  chooseCoordinator("由我协调");
  await screen.findByRole("button", { name: "负责人：由我协调" });
  expect(api.collaborationRoomCoordinator).toHaveBeenLastCalledWith("room-1", "run-1", null, 4, expect.any(String));
});

it("lets the user drop every member and clear the saved coordinator after a stop", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(stoppedRun);
  vi.mocked(api.collaborationRoomStart).mockResolvedValue(activeRun);
  mount();

  // 停止后沿用保存的配置：两人都选中，负责人仍是 s1。
  expect((await screen.findByRole("checkbox", { name: "后端会话" }) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByRole("checkbox", { name: "前端会话" }) as HTMLInputElement).checked).toBe(true);
  expect(screen.getByRole("button", { name: "负责人：后端会话" })).toBeTruthy();

  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  // 取消全部成员后不能被配置悄悄回填。
  await waitFor(() => expect((screen.getByRole("checkbox", { name: "后端会话" }) as HTMLInputElement).checked).toBe(false));
  expect((screen.getByRole("checkbox", { name: "前端会话" }) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("button", { name: "开启协作" }) as HTMLButtonElement).disabled).toBe(true);

  // 重新选人并清除原负责人，再开启时负责人是 null 而不是被回填成 s1。
  fireEvent.click(screen.getByRole("checkbox", { name: "后端会话" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "前端会话" }));
  chooseCoordinator("由我协调");
  await screen.findByRole("button", { name: "负责人：由我协调" });
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

  await waitFor(() => expect(within(screen.getByRole("group", { name: "本次协作" })).getByText("已停止")).toBeTruthy());
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

it("loads the whole room history into the card", async () => {
  const newest = Array.from({ length: 40 }, (_, i) => message(70 + i, `m${70 + i}`));
  const older = Array.from({ length: 40 }, (_, i) => message(30 + i, `m${30 + i}`));
  const oldest = Array.from({ length: 29 }, (_, i) => message(1 + i, `m${1 + i}`));
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(api.collaborationRoomMessages)
    .mockResolvedValueOnce({ items: newest, next_cursor: "cursor-69", latest_seq: 109 })
    .mockResolvedValueOnce({ items: older, next_cursor: "cursor-29", latest_seq: 109 })
    .mockResolvedValueOnce({ items: oldest, next_cursor: null, latest_seq: 109 });
  mount();

  await screen.findByText("m109");
  // 最早一条也在同一个卡里：按游标一路翻到底，不需要用户点。
  expect(await screen.findByText("m1")).toBeTruthy();

  const list = screen.getByRole("list", { name: "协作消息" });
  expect(within(list).getAllByRole("listitem")).toHaveLength(109);
  // 无重叠：跨页出现过的消息只渲染一次。
  expect(within(list).getAllByText("m70")).toHaveLength(1);
  expect(vi.mocked(api.collaborationRoomMessages).mock.calls[1][2]).toBe("cursor-69");
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
  chooseCoordinator("后端会话");
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
  expect(screen.getByText(/开始协作/)).toBeTruthy();
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

it.each([
  { wakeStatus: "pending", label: "等待加入", action: "打开会话", error: null },
  { wakeStatus: "failed", label: "唤醒失败", action: "重试打开", error: "应用未响应" },
] as const)("opens one $wakeStatus member without restarting the run", async ({ wakeStatus, label, action, error }) => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({
    ...activeRun,
    members: [
      member({ session_id: "s1" }),
      member({ session_id: "s2", wake_status: wakeStatus, connection_state: "pending", phase: null, error }),
    ],
  });
  vi.mocked(api.collaborationMemberWake).mockResolvedValue(member({ session_id: "s2" }));
  mount();
  const retried = within(await screen.findByRole("listitem", { name: "前端会话" }));
  expect(retried.getByText(label)).toBeTruthy();
  if (error) expect(retried.getByText(error)).toBeTruthy();
  fireEvent.click(retried.getByRole("button", { name: action }));

  await retried.findByText("等待消息");
  expect(retried.queryByText(label)).toBeNull();
  if (error) expect(retried.queryByText(error)).toBeNull();
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
    within(screen.getByRole("listitem", { name: "后端会话" })).getByText("等待消息"),
  ).toBeTruthy());
  expect(screen.queryByText("等待加入")).toBeNull();
  expect(screen.getAllByRole("listitem").length).toBe(before);
  expect(screen.getByText("先对齐契约")).toBeTruthy();
});

it("shows processing time after joining and expires the connection without another backend event", async () => {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
  const now = new Date("2026-10-27T04:00:00Z");
  vi.setSystemTime(now);
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({
    ...activeRun,
    members: [
      member({
        session_id: "s1", phase: "processing", phase_since: "2026-10-27T03:59:00Z",
        last_heartbeat_at: now.toISOString(), wake_status: "failed", error: "上次打开失败",
      }),
      member({ session_id: "s2", wake_method: "desktop", connection_state: "left" }),
    ],
  });
  mount();
  const terminal = within(await screen.findByRole("listitem", { name: "后端会话" }));
  expect(terminal.getByText("处理中")).toBeTruthy();
  expect(terminal.getByLabelText("已处理 1分钟")).toBeTruthy();
  expect(screen.queryByText("上次打开失败")).toBeNull();

  await act(async () => { vi.advanceTimersByTime(40_000); });
  expect(terminal.getByText("连接中断")).toBeTruthy();
  expect(terminal.queryByLabelText(/已处理/)).toBeNull();
});

it("keeps the chat surface and composer mounted while loading, without a room summary", async () => {
  let resolve!: (view: RoomView) => void;
  vi.mocked(api.collaborationRoomGet).mockReturnValue(new Promise(done => { resolve = done; }));
  mount();
  const history = screen.getByRole("region", { name: "协作消息记录" });
  const composer = screen.getByRole("textbox", { name: "首条消息（随开启一起写入）" });
  expect(history.getAttribute("aria-busy")).toBe("true");
  expect((composer as HTMLTextAreaElement).disabled).toBe(true);
  expect(screen.getByRole("complementary", { name: "任务详情栏" })).toBeTruthy();
  await act(async () => resolve(activeRun));
  await screen.findByText("先对齐契约");
  expect(screen.getByRole("region", { name: "协作消息记录" })).toBe(history);
  expect(screen.getByRole("textbox", { name: "在协作室说一句" })).toBe(composer);
  expect((composer as HTMLTextAreaElement).disabled).toBe(false);
  expect(screen.queryByRole("heading", { name: /协作摘要/ })).toBeNull();
});

it("copies the selected member's labeled NoEnding session ID", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(activeRun);
  vi.mocked(copyToClipboard).mockResolvedValue(true);
  mount();

  const row = within(await screen.findByRole("listitem", { name: "前端会话" }));
  fireEvent.click(row.getByRole("button", { name: "复制会话 ID" }));
  expect(copyToClipboard).toHaveBeenCalledWith("NoEnding ID：s2");
});

it("drops unlinked stopped members and their coordinator before the next start", async () => {
  vi.mocked(api.collaborationRoomGet).mockResolvedValue(stoppedRun);
  const props = { taskId: "task-1", taskTitle: "跨会话聊天室开发", navigate: vi.fn() };
  const page = render(<DetailPanelProvider kind="collaboration"><CollaborationRoomView {...props} sessions={sessions} /></DetailPanelProvider>);
  await screen.findByRole("button", { name: "负责人：后端会话" });
  page.rerender(<DetailPanelProvider kind="collaboration"><CollaborationRoomView {...props} sessions={sessions.filter(session => session.id !== "s1")} /></DetailPanelProvider>);
  expect(screen.getByRole("button", { name: "负责人：由我协调" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "开启协作" })).toHaveProperty("disabled", true);
  expect(within(screen.getByRole("group", { name: "本次协作" })).getByText("已选 1 个")).toBeTruthy();
});
