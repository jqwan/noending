import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionDetailView, { sessionDetailCache, sessionContextCache } from "./SessionDetailView";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type {
  Session,
  SessionContextView,
  LocalDeletePreview,
  SessionDetail,
  SessionMessage,
  SessionWindowMessage,

} from "../../types";

// 只覆盖重构后的详情页：源会话状态、fork 链接、
// 已归档横幅的删除入口、「所属任务」单 Owner 入口，以及 Context 面板。
vi.mock("../../api", () => ({
  api: {
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    terminalList: vi.fn().mockResolvedValue([]),
    getAgentStatus: vi.fn().mockResolvedValue({}),
    launchEmbeddedResume: vi.fn(),
    continueSessionDesktop: vi.fn(),
    getSessionDetail: vi.fn(),
    revealSessionSource: vi.fn(),
    revealSessionMemberSource: vi.fn(),
    getSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      fields: null,
      revision: 0,
      ingest_generation: 0,
      processed_through_seq: 0,
      latest_message_seq: 0,
      updated_at: null,
      pending: false,
    }),
    regenerateSessionContext: vi.fn().mockResolvedValue({ session_id: "", status: "updated", revision: 1, preference_error: null }),
    updateSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      status: "updated",
      revision: 1,
    }),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    archiveSession: vi.fn(),
    restoreSession: vi.fn(),
    setSessionTaskTracking: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
    refreshSession: vi.fn().mockResolvedValue({ queued: true }),
  },
}));

const originalClipboard = navigator.clipboard;
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getSessionTerminalStates).mockImplementation(async ids => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }])));
  vi.mocked(api.terminalList).mockResolvedValue([]);
  vi.mocked(api.getAgentStatus).mockResolvedValue({});
  vi.mocked(api.getSessionContext).mockResolvedValue({ session_id: "", fields: null, revision: 0, ingest_generation: 0, processed_through_seq: 0, latest_message_seq: 0, updated_at: null, pending: false });
  vi.mocked(api.updateSessionContext).mockResolvedValue({ session_id: "", status: "updated", revision: 1, preference_error: null });
  vi.mocked(api.regenerateSessionContext).mockResolvedValue({ session_id: "", status: "updated", revision: 2, preference_error: null });
  vi.mocked(api.listProjects).mockResolvedValue([]);
  vi.mocked(api.listWorkstreams).mockResolvedValue([]);
  vi.mocked(api.refreshSession).mockResolvedValue({ queued: true });
});

afterEach(() => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
  cleanup();
  vi.clearAllMocks();
  // 「显示统计」这类开关活在模块级 view state 里，会跨用例残留。
  viewState.clear();
  sessionDetailCache.clear();
  sessionContextCache.clear();
});

function session(id: string, over: Partial<Session> = {}): Session {
  return {
    id,
    agent: "codex",
    root_agent_session_id: `${id}-agent`,
    title: null,
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    tracked_workstream_ids: [],
    forked_from_session_id: null,
    started_at: null,
    last_activity_at: null,
    last_conversation_at: null,
    archived_at: null,
        source_kind: "codex_rollout",
        source_path: "/tmp/rollout.jsonl",
        metadata: {},
        source_file_identity: "identity",
        source_generation: 1,
        source_byte_offset: 100,
        source_last_seen_size: 100,
        source_mtime: null,
        source_prefix_hash: "",
        source_tail_hash: "",
        fact_generation: 1,
        latest_message_seq: 2,

    ...over,
  };
}


function message(
  sessionId: string,
  sequence: number,
  role: SessionMessage["role"],
  content: string,
): SessionWindowMessage {
  return {
    ordinal: sequence,
    id: `m${sequence}`,
    session_id: sessionId,
    sequence,
    role,
    content,
    ts: null,
    turn_final: true,
    source_message_id: null,
    source_generation: 0,
    source_position: "",
    source_identity_hash: "",
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    messages: [],
    tracked_workstreams: [],
    workspace_path: null,
    ingested_message_sequence: 0,
    processed_message_sequence: 0,
    source_status: "present",
    can_resume: true,
    forked_from: null,
    ...over,
  };
}

async function renderDetail(d: SessionDetail) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);
  const navigate = vi.fn();
  const view = render(<SessionDetailView sessionId={d.session.id} navigate={navigate} />);
  await screen.findByText("会话信息");
  return { navigate, container: document.body, rerender: view.rerender, unmount: view.unmount };
}


it("renders session message stats and tracked tasks in aside", async () => {
  const task1 = {
    id: "w1",
    title: "测试任务",
    description: "任务描述",
    visibility: "normal" as const,
    created_at: "2026-10-13T00:00:00Z",
    updated_at: "2026-10-13T00:00:00Z",
  };
  const d = detail(session("s1", { title: "测试统计会话" }), {
    message_stats: { user_messages: 5, assistant_messages: 8 },
    tracked_workstreams: [task1],
  });
  const { navigate } = await renderDetail(d);

  expect(screen.getByText("会话统计")).toBeTruthy();
  expect(screen.getByText("用户消息 5")).toBeTruthy();
  expect(screen.getByText("Agent 回复 8")).toBeTruthy();
  expect(screen.getByText("跟踪任务（1）")).toBeTruthy();
  expect(screen.getByText("测试任务")).toBeTruthy();
  expect(screen.queryByText("任务描述")).toBeNull();

  fireEvent.click(screen.getByText("测试任务"));
  expect(navigate).toHaveBeenCalledWith({
    view: "workstream",
    workstreamId: "w1",
    initialTitle: "测试任务",
  });
});

// 执行信息

it("reveals the source by clicking the path itself", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("源会话");
  expect(screen.queryByText("源会话已不存在")).toBeNull();
  expect(screen.queryByText("无法确认源会话状态")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "/tmp/rollout.jsonl" }));
  await waitFor(() => expect(api.revealSessionSource).toHaveBeenCalledWith("me"));
});

it("renders separate terminal and desktop actions for codex session even if source is missing", async () => {
  await renderDetail(detail(session("me"), { source_status: "missing", can_resume: false }));

  screen.getByText("源会话已不存在");
  // Codex supports both entries, which are separate titlebar controls.
  const resume = screen.getByRole("button", { name: "在桌面应用中继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(false);


  expect(screen.getByRole("button", { name: "在终端中继续" })).toBeTruthy();

  // 源不在，路径退回纯文本：没有可点的定位入口，只有警告。
  expect(screen.queryByRole("button", { name: "/tmp/rollout.jsonl" })).toBeNull();
});

it("shows only terminal continue button for terminal-only formats and desktop for desktop-only", async () => {
  // Claude Code: terminal only
  const { unmount } = await renderDetail(detail(session("claude-1", { agent: "claude_code" })));
  expect(screen.getByRole("button", { name: "在终端中继续" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "在桌面应用中继续" })).toBeNull();
  expect(screen.queryByRole("button", { name: "切换继续方式" })).toBeNull();
  unmount();

  // Antigravity Desktop: desktop only
  await renderDetail(detail(session("ag-1", { agent: "antigravity", source_kind: "antigravity_desktop" })));
  expect(screen.getByRole("button", { name: "在桌面应用中继续" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "在终端中继续" })).toBeNull();
  expect(screen.queryByRole("button", { name: "切换继续方式" })).toBeNull();
});

// 消息

it("previews the newest messages and links to the whole conversation", async () => {
  const me = session("me");
  const { navigate } = await renderDetail(detail(me, {
    messages: [{ ...message("me", 1173, "user", "最近的一句话"), ordinal: 419 }],
    ingested_message_sequence: 420,
  }));

  screen.getByText("最近消息");
  screen.getByText("最近的一句话");
  expect(screen.getByText("#419")).toBeTruthy();
  expect(screen.queryByText("#1173")).toBeNull();
  expect(screen.queryByText(/以上是最近/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "查看全部历史消息（共 420 条）" }));
  expect(navigate).toHaveBeenCalledWith({
    view: "session",
    sessionId: "me",
    entry: "conversation",
    initialTitle: "未命名会话",
    initialAgent: "codex",
    initialTotal: 420,
  });
});

it("offers no conversation entry for a session that has no messages", async () => {
  await renderDetail(detail(session("me")));

  expect(screen.queryByRole("button", { name: /查看全部历史消息/ })).toBeNull();
  screen.getByText(/还没有同步消息/);
});

// Fork

it("handles fork origin provenance: linked session, missing source, and non-fork", async () => {
  // 1. Linked source session
  const meWithFork = session("me", { forked_from_session_id: "orig" });
  const { navigate, unmount } = await renderDetail(detail(meWithFork, {
    forked_from: session("orig", { title: "原始会话" }),
  }));
  fireEvent.click(screen.getByRole("button", { name: "分叉自：原始会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "orig", initialTitle: "原始会话", initialAgent: "codex" });
  unmount();

  // 2. Missing source session
  const meWithMissingFork = session("me", { forked_from_session_id: "ghost" });
  const { unmount: unmountMissing } = await renderDetail(detail(meWithMissingFork));
  screen.getByText(/来源会话不在 NoEnding 库里/);
  screen.getByText(/ghost/);
  unmountMissing();

  // 3. Not a fork
  await renderDetail(detail(session("me")));
  expect(screen.queryByText(/分叉自/)).toBeNull();
});

// 已归档横幅

function preview(over: Partial<LocalDeletePreview> = {}): LocalDeletePreview {
  return {
    session_id: "me",
    session_title: "会话一",
    agent: "codex",
    root_agent_session_id: "me-agent",
    root_source_status: "missing",
    message_count: 3,
    session_context_count: 0,

    ...over,
  };
}

function archivedDetail(over: Partial<SessionDetail> = {}): SessionDetail {
  return detail(session("me", { archived_at: "2026-09-24T00:00:00+00:00" }), {
    can_resume: false,
    ...over,
  });
}


// 点删除之后：先查源状态，告知这次是彻底删除还是会被重新入库，用户再确认。

it("checks root source status in local delete preview", async () => {
  // 1. Root source present
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "present" }),
  );
  const { unmount } = await renderDetail(archivedDetail({ source_status: "present" }));
  expect(screen.queryByText("该会话已归档")).toBeNull();
  expect(screen.getByText("归档时间")).toBeDefined();
  screen.getByRole("button", { name: "取消归档" });
  fireEvent.click(screen.getByRole("button", { name: "永久删除" }));
  await screen.findByText(/Root 源会话仍然存在/);
  screen.getByText(/下一次同步会从它重新同步/);
  screen.getByText("3 条会话消息");
  screen.getByText("0 条 Context 摘要记录");
  screen.getByRole("button", { name: "删除" });
  unmount();

  // 2. Root source missing
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "missing" }),
  );
  await renderDetail(archivedDetail({ source_status: "missing" }));
  fireEvent.click(screen.getByRole("button", { name: "永久删除" }));
  await screen.findByText(/本地数据删除后无法找回/);
  expect(screen.queryByText(/Root 源会话仍然存在/)).toBeNull();
});

// Context 面板

function contextView(over: Partial<SessionContextView> = {}): SessionContextView {
  return {
    session_id: "me",
    fields: null,
    revision: 0,
    ingest_generation: 0,
    processed_through_seq: 0,
    latest_message_seq: 0,
    updated_at: null,
    pending: false,
    ...over,
  };
}

it("renders summary, supports copying, and updates context on pending increment", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary: { items: [{ title: "前端入口重构", description: "**正在重构前端入口**，删掉 Context Intelligence 开关。", details: ["下一步补测试。"] }] },
    },
    pending: true,
    revision: 3,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "继续")],
  }));

  screen.getByText("会话摘要（1）");
  expect(screen.getByText("正在重构前端入口").tagName).toBe("STRONG");
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  fireEvent.click(screen.getByRole("button", { name: "复制摘要" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "### 前端入口重构\n\n**正在重构前端入口**，删掉 Context Intelligence 开关。\n\n- 下一步补测试。",
  ));
  screen.getByText(/删掉 Context Intelligence 开关/);
  screen.getByText(/补测试/);
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
});

it("allows regenerating a current summary without pending messages", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary: { items: [{ title: "已完成", description: "", details: [] }] },
    },
    pending: false,
    revision: 1,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "好了")],
  }));

  expect(screen.queryByRole("button", { name: "更新摘要" })).toBeNull();
  let finish!: (value: { session_id: string; status: "updated"; revision: number; preference_error: string | null }) => void;
  vi.mocked(api.regenerateSessionContext).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const regenerate = screen.getByRole("button", { name: "重新生成" });
  fireEvent.click(regenerate);
  expect(regenerate).toHaveProperty("disabled", true);
  await waitFor(() => expect(api.regenerateSessionContext).toHaveBeenCalledWith("me"));
  expect(api.updateSessionContext).not.toHaveBeenCalled();
  await act(async () => finish({ session_id: "me", status: "updated", revision: 2, preference_error: null }));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
});

it("shows and copies structured Context failure details, retains across sync, and clears on session navigation", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });

  const { navigate, rerender } = await renderDetail(detail(session("session-a"), {
    messages: [message("session-a", 1, "user", "开始吧")],
  }));
  expect(screen.queryByRole("button", { name: "复制摘要" })).toBeNull();

  // 1. Shows error and copies error details
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);
  fireEvent.click(screen.getByRole("button", { name: "复制错误详情" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "内容已变化，请重新更新\n错误代码：stale_snapshot\n操作 ID：123e4567-e89b-12d3-a456-426614174000",
  ));

  // 2. Retains error across background sync
  window.dispatchEvent(new Event("noending:sync"));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
  expect(screen.getByText(failure)).toBeTruthy();

  // 3. Clears error on session navigation
  rerender(<SessionDetailView sessionId="session-b" navigate={navigate} />);
  await waitFor(() => expect(screen.queryByText(failure)).toBeNull());
});

it("triggers incremental scan when clicking the refresh button in header", async () => {
  await renderDetail(detail(session("me")));

  const refreshBtn = screen.getByRole("button", { name: "增量同步" });
  expect(refreshBtn).toBeTruthy();
  fireEvent.click(refreshBtn);
  await waitFor(() => expect(api.refreshSession).toHaveBeenCalledWith("me"));
});

it("renders immediately from cache on remount without flashing 加载中", async () => {
  const me = session("me", { title: "已缓存的会话" });
  await renderDetail(detail(me));
  expect(screen.getByText("已缓存的会话")).toBeDefined();

  cleanup();

  // On remount (e.g. returning from conversation):
  // Delay api.getSessionDetail to verify cached detail is rendered immediately
  let resolveDetail: (d: any) => void;
  vi.mocked(api.getSessionDetail).mockReturnValue(new Promise((r) => { resolveDetail = r; }) as any);

  render(<SessionDetailView sessionId="me" navigate={vi.fn()} />);

  // Should render "已缓存的会话" immediately, NOT "加载中…"
  expect(screen.queryByText("加载中…")).toBeNull();
  expect(screen.getByText("已缓存的会话")).toBeDefined();

  await act(async () => {
    resolveDetail!(detail(me));
  });
});

it("renders cached summary immediately without flashing 暂无摘要", async () => {
  const me = session("me", { title: "带摘要的会话" });
  const d = detail(me);
  sessionDetailCache.set("me", d);
  sessionContextCache.set("me", contextView({
    fields: {
      summary: { items: [{ title: "这是已缓存的会话摘要", description: "", details: [] }] },
    },
  }));
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);

  render(<SessionDetailView sessionId="me" navigate={vi.fn()} />);

  expect(screen.getByText("这是已缓存的会话摘要")).toBeDefined();
  expect(screen.getByRole("button", { name: "复制摘要" })).toBeDefined();
});

it("renders copy icon buttons next to field labels and copies values on click", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });

  const me = session("session-123", {
    root_agent_session_id: "agent-root-456",
    source_path: "/path/to/source.jsonl",
  });
  await renderDetail(detail(me));

  // Copy icon button for "会话 ID"
  const copySessionIdBtn = screen.getByRole("button", { name: "复制会话 ID" });
  expect(copySessionIdBtn).toBeDefined();
  fireEvent.click(copySessionIdBtn);
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("session-123"));

  // Copy icon button for "Agent 会话 ID"
  const copyAgentIdBtn = screen.getByRole("button", { name: "复制 Agent 会话 ID" });
  expect(copyAgentIdBtn).toBeDefined();
  fireEvent.click(copyAgentIdBtn);
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("agent-root-456"));

  // Copy icon button for "源会话"
  const copySourceBtn = screen.getByRole("button", { name: "复制源会话路径" });
  expect(copySourceBtn).toBeDefined();
  fireEvent.click(copySourceBtn);
  // 会话统计不再提供复制按钮
  expect(screen.queryByRole("button", { name: "复制会话统计" })).toBeNull();
});

it("allows archived sessions to sync and generate summaries while blocking continue", async () => {
  vi.mocked(api.updateSessionContext).mockResolvedValue({ status: "updated" } as never);
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  await renderDetail(archivedDetail({ messages: [message("me", 1, "user", "归档后依然可生成摘要")] }));
  screen.getByRole("button", { name: "增量同步" });
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  expect((screen.getByRole("button", { name: "在桌面应用中继续" }) as HTMLButtonElement).disabled).toBe(true);
  const terminal = screen.getByRole("button", { name: "在终端中继续" });
  expect((terminal as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(terminal);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.continueSessionDesktop).not.toHaveBeenCalled();
});

it("keeps the known session title throughout a delayed detail load", async () => {
  let resolve!: (value: SessionDetail) => void;
  vi.mocked(api.getSessionDetail).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const navigate = vi.fn();
  render(<SessionDetailView sessionId="s1" initialTitle="稳定标题" initialAgent="codex" navigate={navigate} />);
  expect(screen.getByRole("heading").textContent).toBe("稳定标题");
  fireEvent.click(screen.getByRole("button", { name: "历史消息" }));
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ initialTitle: "稳定标题", initialAgent: "codex" }));
  await act(async () => resolve(detail(session("s1", { title: "稳定标题" }))));
  expect(screen.getByRole("heading").textContent).toBe("稳定标题");
});

it("blocks continue when working directory does not exist", async () => {
  vi.mocked(api.getSessionTerminalStates).mockResolvedValue({ "missing-dir": { available: false, reason: "工作目录不存在，无法继续会话" } });
  const missingDirSession = session("missing-dir", { cwd: "/deleted/path" });
  const d = detail(missingDirSession, {
    can_resume: false,
    workspace_path: {
      id: "wp1",
      canonical_path: "/deleted/path",
      exists: false,
      project_id: "p1",
      project_name: "Deleted Project",
    },
  });
  await renderDetail(d);

  const desktopBtn = screen.getByRole("button", { name: "在桌面应用中继续" }) as HTMLButtonElement;
  expect(desktopBtn.disabled).toBe(true);
  expect(desktopBtn.title).toBe("工作目录不存在，无法继续会话");

  const terminalBtn = screen.getByRole("button", { name: "在终端中继续" }) as HTMLButtonElement;
  expect(terminalBtn.disabled).toBe(true);
  expect(terminalBtn.title).toBe("工作目录不存在，无法继续会话");

  fireEvent.click(terminalBtn);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.continueSessionDesktop).not.toHaveBeenCalled();
});

it("shows the physical compressed source and uses it for copy and reveal", async () => {
  const path = "/path/to/rollout.jsonl.zst";
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await renderDetail(detail(session("compressed", { agent: "codex", source_path: path })));
  expect(screen.getByText("Zstd 压缩")).toBeDefined();
  fireEvent.click(screen.getByRole("button", { name: path }));
  await waitFor(() => expect(api.revealSessionSource).toHaveBeenCalledWith("compressed"));
  fireEvent.click(screen.getByRole("button", { name: "复制源会话路径" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(path));
});
