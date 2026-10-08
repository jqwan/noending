import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionDetailView, { sessionDetailCache } from "./SessionDetailView";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type {
  Session,
  SessionContextView,
  LocalDeletePreview,
  SessionDetail,
  SessionMessage,
  Workstream,
} from "../../types";

// 只覆盖重构后的详情页：源会话状态、fork 链接、
// 已归档横幅的删除入口、「所属任务」单 Owner 入口，以及 Context 面板。
vi.mock("../../api", () => ({
  api: {
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
    updateSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      status: "updated",
      revision: 1,
    }),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    archiveSession: vi.fn(),
    restoreSession: vi.fn(),
    setSessionOwnerWorkstream: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
    refreshSession: vi.fn().mockResolvedValue({ queued: true }),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  // 「显示统计」这类开关活在模块级 view state 里，会跨用例残留。
  viewState.clear();
  sessionDetailCache.clear();
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
    owner_workstream_id: null,
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

function workstream(id: string, title: string): Workstream {
  return {
    id,
    title,
    description: "",
    visibility: "normal",
    created_at: "2026-09-21T00:00:00+00:00",
    updated_at: "2026-09-21T00:00:00+00:00",
  };
}

function message(
  sessionId: string,
  sequence: number,
  role: SessionMessage["role"],
  content: string,
): SessionMessage {
  return {
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
    raw_ref: "",
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    messages: [],
    owner_workstream: null,
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
  const view = render(<SessionDetailView sessionId={d.session.id} navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("会话信息");
  return { navigate, container: document.body, rerender: view.rerender, unmount: view.unmount };
}

it("renders agent icon before session title and plain agent name under session info", async () => {
  const d = detail(session("s1", { title: "测试会话标题", agent: "codex" }));
  await renderDetail(d);

  // Title in header has session-title-with-icon containing agent icon and title text
  const titleContainer = document.querySelector(".session-title-with-icon");
  expect(titleContainer).toBeTruthy();
  expect(titleContainer?.querySelector(".agent-icon")).toBeTruthy();
  expect(titleContainer?.textContent).toContain("测试会话标题");

  // In aside section "会话信息":
  const aside = document.querySelector(".task-detail-aside");
  expect(aside).toBeTruthy();
  // Shows agent label "Codex"
  expect(aside?.textContent).toContain("Codex");
  // But has NO agent icon inside aside
  expect(aside?.querySelector(".agent-icon")).toBeNull();
});

it("renders session message stats in session info aside", async () => {
  const d = detail(session("s1", { title: "测试统计会话" }), {
    message_stats: { user_messages: 5, assistant_messages: 8 },
  });
  await renderDetail(d);

  expect(screen.getByText("会话统计")).toBeTruthy();
  expect(screen.getByText("用户消息 5")).toBeTruthy();
  expect(screen.getByText("代理回复 8")).toBeTruthy();
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

it("renders aggregate continue button for codex session even if source is missing", async () => {
  await renderDetail(detail(session("me"), { source_status: "missing", can_resume: false }));

  screen.getByText("源会话已不存在");
  // 聚合按钮在场（codex 兼具终端与桌面能力）
  expect(screen.getByRole("button", { name: "切换继续方式" })).toBeTruthy();
  const resume = screen.getByRole("button", { name: "在桌面应用中继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(false);

  // 展开切换菜单能看到在终端中继续选项
  fireEvent.click(screen.getByRole("button", { name: "切换继续方式" }));
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
    messages: [message("me", 419, "user", "最近的一句话")],
    ingested_message_sequence: 420,
  }));

  screen.getByText("最近的一句话");
  screen.getByText("以上是最近 1 条。");
  fireEvent.click(screen.getByRole("button", { name: "查看全部会话（共 420 条）" }));
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

  expect(screen.queryByRole("button", { name: /查看全部会话/ })).toBeNull();
  screen.getByText(/还没有同步消息/);
});

// Fork

it("links to the session it forked from", async () => {
  const me = session("me", { forked_from_session_id: "orig" });
  const { navigate } = await renderDetail(detail(me, {
    forked_from: session("orig", { title: "原始会话" }),
  }));

  fireEvent.click(screen.getByRole("button", { name: "分叉自：原始会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "orig" });
});

it("names the missing fork source instead of hiding the provenance", async () => {
  const me = session("me", { forked_from_session_id: "ghost" });
  await renderDetail(detail(me));

  screen.getByText(/来源会话不在 NoEnding 库里/);
  screen.getByText(/ghost/);
});

it("shows no fork field for a session that is not a fork", async () => {
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
    launch_intent_count: 0,
    context_revision_redaction_count: 0,
    ...over,
  };
}

function archivedDetail(over: Partial<SessionDetail> = {}): SessionDetail {
  return detail(session("me", { archived_at: "2026-09-24T00:00:00+00:00" }), {
    can_resume: false,
    ...over,
  });
}

it("offers the same 删除 entry whatever the root source state", async () => {
  // Trash is the only gate: the source verdict never renames or hides the entry.
  for (const source_status of ["missing", "present"] as const) {
    cleanup();
    await renderDetail(archivedDetail({ source_status }));
    screen.getByRole("button", { name: "永久删除…" });
    expect(screen.queryByText(/重新入库…/)).toBeNull();
    expect(screen.queryByText(/删除不可用/)).toBeNull();
  }
});

// 点删除之后：先查源状态，告知这次是彻底删除还是会被重新入库，用户再确认。

it("checks the root source before confirming and says the copy will be rebuilt", async () => {
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "present" }),
  );
  await renderDetail(archivedDetail({ source_status: "present" }));

  fireEvent.click(screen.getByRole("button", { name: "永久删除…" }));

  await screen.findByText(/Root 源会话仍然存在/);
  screen.getByText(/下一次同步会从它重新同步/);
  // 每一条「将删除」都渲染出数字：后端改名而前端漏改时，这里会当场炸掉，
  // 而不是在生产里把整棵树渲染崩成黑屏。
  screen.getByText("3 条会话消息");
  screen.getByText("0 条 Context 摘要记录");
  screen.getByText("0 条启动记录");
  screen.getByText("上下文来源改写 0 条");
  screen.getByRole("button", { name: "删除" });
});

it("says the local data is unrecoverable when the root source is gone", async () => {
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "missing" }),
  );
  await renderDetail(archivedDetail({ source_status: "missing" }));

  fireEvent.click(screen.getByRole("button", { name: "永久删除…" }));

  await screen.findByText(/本地数据删除后无法找回/);
  expect(screen.queryByText(/Root 源会话仍然存在/)).toBeNull();
});

// 所属任务（单 Owner）

it("shows the one owner workstream and links to it", async () => {
  const owner = workstream("w1", "会话与 Workstream 重构");
  const { navigate } = await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), {
    owner_workstream: owner,
  }));

  screen.getByText("所属任务");
  screen.getByText(owner.title);
  // 点击整栏直接跳转
  const row = document.querySelector(".rail-row") as HTMLElement;
  expect(row).toBeTruthy();
  fireEvent.click(row);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });

  // 只有一个所属任务：没有「添加」多任务入口。
  expect(screen.queryByRole("button", { name: /添加/ })).toBeNull();
});

it("marks an unowned session and offers 选择 instead of 添加", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("所属任务");
  screen.getByText("未归属任务");
  screen.getByRole("button", { name: "选择" });
  expect(screen.queryByRole("button", { name: "更改" })).toBeNull();
});

it("replaces the owner through a single-choice modal", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner, workstream("w2", "新任务")]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  const dialog = await screen.findByRole("dialog", { name: "选择所属任务" });
  // 单选：一次只能选一个。
  const radios = dialog.querySelectorAll('input[type="radio"]');
  expect(radios).toHaveLength(3);
  fireEvent.click(screen.getByText("新任务"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", "w2"));
});

it("clears the owner by choosing 未归属", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  await screen.findByRole("dialog", { name: "选择所属任务" });
  fireEvent.click(screen.getByText("未归属"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", null));
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

it("offers 生成摘要 only when there are messages and no summary yet", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  screen.getByText("Context");
  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  // 更新成功后重新拉取 Context 与详情。
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
});

it("keeps the summary read-only and shows 更新摘要 while there is a pending increment", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "正在重构前端入口",
      decisions: ["删掉 Context Intelligence 开关"],
      open_questions: [],
      next_steps: ["补测试"],
    },
    pending: true,
    revision: 3,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "继续")],
  }));

  screen.getByText(/正在重构前端入口/);
  screen.getByText(/删掉 Context Intelligence 开关/);
  screen.getByText(/补测试/);
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
});

it("hides the update button when there is nothing pending", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "已完成",
      decisions: [],
      open_questions: [],
      next_steps: [],
    },
    pending: false,
    revision: 1,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "好了")],
  }));

  expect(screen.queryByRole("button", { name: "更新摘要" })).toBeNull();
  expect(screen.queryByRole("button", { name: "生成摘要" })).toBeNull();
});

it("shows structured Context failure details and copies the operation id", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue(
    { code: "stale_snapshot", message: "内容已变化，请重新更新", operation_id: "123e4567-e89b-12d3-a456-426614174000" },
  );
  const originalClipboard = navigator.clipboard;
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await screen.findByText("内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000");
  fireEvent.click(screen.getByRole("button", { name: "复制错误详情" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "内容已变化，请重新更新\n错误代码：stale_snapshot\n操作 ID：123e4567-e89b-12d3-a456-426614174000",
  ));
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
});

it("keeps an update error visible across background sync refreshes", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  window.dispatchEvent(new Event("noending:sync"));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
  expect(screen.getByText(failure)).toBeTruthy();
});

it("clears a session's update error when the same detail view navigates to another session", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  const { navigate, rerender } = await renderDetail(detail(session("session-a"), {
    messages: [message("session-a", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  rerender(<SessionDetailView sessionId="session-b" navigate={navigate} goBack={vi.fn()} />);
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

  render(<SessionDetailView sessionId="me" navigate={vi.fn()} goBack={vi.fn()} />);

  // Should render "已缓存的会话" immediately, NOT "加载中…"
  expect(screen.queryByText("加载中…")).toBeNull();
  expect(screen.getByText("已缓存的会话")).toBeDefined();

  await act(async () => {
    resolveDetail!(detail(me));
  });
});

it("renders copy icon buttons next to field labels and copies values on click", async () => {
  const originalClipboard = navigator.clipboard;
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
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("/path/to/source.jsonl"));

  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
});

it("allows archived sessions to sync and generate summaries while blocking continue", async () => {
  vi.mocked(api.updateSessionContext).mockResolvedValue({ status: "updated" } as never);
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  await renderDetail(archivedDetail({ messages: [message("me", 1, "user", "归档后依然可生成摘要")] }));
  screen.getByRole("button", { name: "增量同步" });
  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  expect((screen.getByRole("button", { name: "在桌面应用中继续" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "切换继续方式" }));
  const terminal = screen.getByRole("button", { name: "在终端中继续" });
  expect((terminal as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(terminal);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.continueSessionDesktop).not.toHaveBeenCalled();
});
