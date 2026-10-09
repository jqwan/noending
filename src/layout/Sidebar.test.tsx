// Sidebar 契约：一等导航 + 「运行中」终端列表（registry 运行时事实，事件刷新）。
// 旧的最近任务/固定列表已由运行终端取代。

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
import { api } from "../api";
import { disposeTerminal } from "../features/sessions/terminalCache";
import { EVT_TERMINALS, EVT_SYNCED } from "../app/routes";
import type { Route } from "../app/routes";
import type { TerminalSummary, Session, WorkstreamCardData, ProjectCardData } from "../types";

vi.mock("../api", () => ({
  api: {
    terminalList: vi.fn(),
    terminalClose: vi.fn(),
    listSessions: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listProjectCards: vi.fn(),
  },
}));
vi.mock("../features/sessions/terminalCache", () => ({ disposeTerminal: vi.fn() }));

beforeEach(() => {
  vi.mocked(disposeTerminal).mockClear();
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
  vi.mocked(api.terminalClose).mockReset().mockImplementation(async (terminalId) => terminal({ terminal_id: terminalId }));
  vi.mocked(api.listSessions).mockReset().mockResolvedValue([]);
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

function renderSidebar(route: Route = { view: "new-session" }, navigate = vi.fn()) {
  render(<Sidebar route={route} navigate={navigate} onSearch={() => {}} />);
  return navigate;
}

function terminal(over: Partial<TerminalSummary>): TerminalSummary {
  return {
    terminal_id: "t-1",
    session_id: null,
    identity_revision: 0,
    agent: "codex",
    cwd: "/repo/x",
    created_at: new Date().toISOString(),
    live: true,
    exit_code: null,
    session_title: null,
    ...over,
  };
}

function session(over: Partial<Session> = {}): Session {
  const baseTime = over.last_conversation_at ?? over.last_activity_at ?? new Date(Date.now() - 3600 * 1000).toISOString();
  return {
    id: "s-1",
    agent: "codex",
    root_agent_session_id: "agent-1",
    title: "修复侧栏问题",
    cwd: "/repo/x",
    project_id: "p-1",
    workspace_path_id: "wp-1",
    owner_workstream_id: "ws-1",
    forked_from_session_id: null,
    started_at: baseTime,
    last_activity_at: baseTime,
    last_conversation_at: baseTime,
    archived_at: null,
    source_kind: "file",
    source_path: "/path/to/source",
    metadata: {},
    source_file_identity: "file-id",
    source_generation: 1,
    source_byte_offset: 100,
    source_last_seen_size: 100,
    source_mtime: Date.now(),
    source_prefix_hash: "prefix-hash",
    source_tail_hash: "tail-hash",
    fact_generation: 1,
    latest_message_seq: 1,
    ...over,
  };
}

function workstreamCard(over: Partial<WorkstreamCardData> = {}): WorkstreamCardData {
  return {
    id: "ws-1",
    projects: [{ id: "p-1", name: "项目A" }],
    title: "任务重构",
    description: "重构描述",

    visibility: "normal",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    current_state: null,
    goal: null,
    last_activity_at: new Date().toISOString(),
    session_count: 1,
    latest_session: null,
    path_count: 1,

    ...over,
  };
}

function projectCard(over: Partial<ProjectCardData> = {}): ProjectCardData {
  return {
    id: "p-1",
    name: "项目A",
    name_customized: false,
    kind: "directory" as const,
    path_count: 1,
    missing_path_count: 0,
    workstream_count: 1,
    session_count: 1,
    representative_paths: ["/repo/x"],
    search_paths: ["/repo/x"],
    last_activity_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...over,
  };
}

describe("Sidebar navigation", () => {
  it("opens the new-session page", async () => {
    const navigate = renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalled());
    const newSession = screen.getByRole("button", { name: "新会话" });
    expect(newSession.className).toContain("active");
    fireEvent.click(newSession);
    expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  });

  it("preserves the search entry", async () => {
    const onSearch = vi.fn();
    render(<Sidebar route={{ view: "new-session" }} navigate={vi.fn()} onSearch={onSearch} />);
    fireEvent.click(await screen.findByRole("button", { name: /搜索/ }));
    expect(onSearch).toHaveBeenCalledOnce();
  });
});

describe("Sidebar 运行中终端", () => {
  it("keeps exited terminals visible and clickable with a muted icon and title", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ live: false, exit_code: 0, session_id: "s1", session_title: "已结束的会话" }),
    ]);
    const navigate = renderSidebar();
    const row = await screen.findByRole("button", { name: "已结束的会话" });
    expect(screen.getByText("运行中")).toBeTruthy();
    expect(row.className).toContain("terminal-exited");
    expect(row.title).toContain("已退出");
    expect(row.querySelector(".agent-icon")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /重新连接/ })).toBeNull();
    fireEvent.click(row);
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-1" });
  });

  it("mutes a terminal after its exit event without removing its sidebar entry", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([terminal({})]);
    renderSidebar();
    expect((await screen.findByTitle("新会话 · /repo/x")).className).not.toContain("terminal-exited");
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ live: false, exit_code: 0 })]);
    act(() => window.dispatchEvent(new CustomEvent(EVT_TERMINALS)));
    const row = await screen.findByTitle("新会话 · /repo/x · 已退出");
    expect(row.className).toContain("terminal-exited");
    expect(screen.getByRole("button", { name: "移除终端：新会话" })).toBeTruthy();
  });

  it("manually removing an exited terminal clears its entry and frontend cache", async () => {
    const exited = terminal({ live: false, session_id: "s1", session_title: "旧会话" });
    vi.mocked(api.terminalList).mockResolvedValue([exited]);
    vi.mocked(api.terminalClose).mockResolvedValue(exited);
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-1" });
    fireEvent.click(await screen.findByRole("button", { name: "移除终端：旧会话" }));
    await waitFor(() => expect(disposeTerminal).toHaveBeenCalledWith("t-1"));
    expect(api.terminalClose).toHaveBeenCalledWith("t-1");
    expect(screen.queryByText("旧会话")).toBeNull();
    expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1" });
  });

  it("hides the section when no terminal is running", async () => {
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalled());
    expect(screen.queryByText("运行中")).toBeNull();
  });

  it("shows live terminals; unbound reads 新会话, bound reads the session name", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-1" }),
      terminal({ terminal_id: "t-2", session_id: "s1", session_title: "修复布局" }),
    ]);
    const navigate = renderSidebar();

    expect(await screen.findByTitle("新会话 · /repo/x")).toBeTruthy();
    expect(screen.getByText("修复布局")).toBeTruthy();

    fireEvent.click(screen.getByTitle("新会话 · /repo/x"));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-1" });
    fireEvent.click(screen.getByText("修复布局"));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-2" });
  });

  it("closing the unbound terminal on screen goes back to the sessions board", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "t-1" })]);
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-1" });
    expect(await screen.findByTitle("新会话 · /repo/x")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：新会话" }));
    await waitFor(() => expect(api.terminalClose).toHaveBeenCalledWith("t-1"));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "sessions" }));
  });

  it("closing a bound terminal on screen goes to its session detail", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-2", session_id: "s1", session_title: "修复布局" }),
    ]);
    vi.mocked(api.terminalClose).mockResolvedValue(terminal({ terminal_id: "t-2", session_id: "s1", identity_revision: 1 }));
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-2" });
    expect(await screen.findByText("修复布局")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：修复布局" }));
    await waitFor(() => expect(api.terminalClose).toHaveBeenCalledWith("t-2"));
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1" }),
    );
  });

  it("closing a terminal that is not on screen does not navigate", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "t-1" })]);
    const navigate = renderSidebar({ view: "new-session" });
    expect(await screen.findByTitle("新会话 · /repo/x")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：新会话" }));
    await waitFor(() => expect(api.terminalClose).toHaveBeenCalledWith("t-1"));
    await waitFor(() => expect(navigate).not.toHaveBeenCalled());
  });

  it("refetches once per terminals-changed event — no polling", async () => {
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(1));

    window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
    window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(3));
  });

  it("keeps each terminal visible when several terminals share a session", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-1", session_id: "s1", session_title: "同一个会话", identity_revision: 1 }),
      terminal({ terminal_id: "t-2", session_id: "s1", session_title: "同一个会话", identity_revision: 1 }),
    ]);
    const navigate = renderSidebar();
    const rows = await screen.findAllByText("同一个会话");
    expect(rows).toHaveLength(2);
    rows.forEach((row) => fireEvent.click(row));
    expect(navigate).toHaveBeenNthCalledWith(1, { view: "terminal", terminalId: "t-1" });
    expect(navigate).toHaveBeenNthCalledWith(2, { view: "terminal", terminalId: "t-2" });
  });

  it("ignores an old list response after a newer identity refresh", async () => {
    let resolveOld!: (rows: TerminalSummary[]) => void;
    vi.mocked(api.terminalList)
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce([terminal({ session_id: "b", session_title: "会话 B", identity_revision: 2 })]);
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(1));
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await screen.findByText("会话 B");
    await act(async () => { resolveOld([terminal({ session_id: "a", session_title: "旧会话 A", identity_revision: 1 })]); });
    expect(screen.getByText("会话 B")).toBeTruthy();
    expect(screen.queryByText("旧会话 A")).toBeNull();
  });

  it("navigates using the identity returned by close instead of the displayed row", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "a", session_title: "会话 A", identity_revision: 1 })]);
    vi.mocked(api.terminalClose).mockResolvedValue(terminal({ session_id: "b", session_title: "会话 B", identity_revision: 2 }));
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-1" });
    await screen.findByText("会话 A");
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：会话 A" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "b" }));
  });

  it("returns to the sessions board when close reports a pending identity", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "a", session_title: "会话 A", identity_revision: 1 })]);
    vi.mocked(api.terminalClose).mockResolvedValue(terminal({ session_id: null, identity_revision: 2 }));
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-1" });
    await screen.findByText("会话 A");
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：会话 A" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "sessions" }));
  });

  it("does not navigate after the user leaves while close is pending", async () => {
    let finishClose!: (summary: TerminalSummary) => void;
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "a", session_title: "会话 A", identity_revision: 1 })]);
    vi.mocked(api.terminalClose).mockReturnValue(new Promise((resolve) => { finishClose = resolve; }));
    const navigate = vi.fn();
    const view = render(<Sidebar route={{ view: "terminal", terminalId: "t-1" }} navigate={navigate} onSearch={() => {}} />);
    await screen.findByText("会话 A");
    fireEvent.click(screen.getByRole("button", { name: "关闭终端：会话 A" }));
    view.rerender(<Sidebar route={{ view: "new-session" }} navigate={navigate} onSearch={() => {}} />);
    await act(async () => { finishClose(terminal({ session_id: "b", identity_revision: 2 })); });
    expect(navigate).not.toHaveBeenCalled();
  });
});

describe("Sidebar 最近活动", () => {
  it("renders empty state when there are no sessions within the last 7 days", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-old",
        title: "较旧的会话",
        last_conversation_at: new Date(Date.now() - 8 * 24 * 60 * 60 * 1000).toISOString(),
      }),
    ]);

    renderSidebar();
    expect(await screen.findByText("最近 7 天无新消息")).toBeTruthy();
    expect(screen.queryByText("较旧的会话")).toBeNull();
  });

  it("filters sessions to last 7 days and excludes trashed sessions", async () => {
    const activeRecent = session({
      id: "s-recent",
      title: "近期活跃会话",
      last_conversation_at: new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString(),
    });
    const oldSession = session({
      id: "s-old",
      title: "过期会话",
      last_conversation_at: new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString(),
    });
    const trashedSession = session({
      id: "s-trash",
      title: "已删除会话",
      last_conversation_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
      archived_at: new Date().toISOString(),
    });

    vi.mocked(api.listSessions).mockResolvedValue([activeRecent, oldSession, trashedSession]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "测试任务" }),
    ]);

    renderSidebar();
    expect(await screen.findByText("近期活跃会话")).toBeTruthy();
    expect(screen.queryByText("过期会话")).toBeNull();
    expect(screen.queryByText("已删除会话")).toBeNull();
  });

  it("groups by workstream by default and navigates to session detail on click", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "修复侧栏问题",
        owner_workstream_id: "ws-1",
        last_conversation_at: new Date(Date.now() - 1000).toISOString(),
      }),
    ]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "侧栏任务" }),
    ]);

    const navigate = renderSidebar();
    expect(await screen.findByText("侧栏任务")).toBeTruthy();
    const sessionItem = screen.getByText("修复侧栏问题");

    fireEvent.click(sessionItem);
    expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s-1" });
  });

  it("toggles group collapse when group header is clicked", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "可折叠会话",
        owner_workstream_id: "ws-1",
      }),
    ]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "折叠测试任务" }),
    ]);

    renderSidebar();
    const groupHeader = await screen.findByRole("button", { name: /折叠测试任务/ });
    expect(screen.getByText("可折叠会话")).toBeTruthy();

    // 点击收起
    fireEvent.click(groupHeader);
    expect(screen.queryByText("可折叠会话")).toBeNull();

    // 再次点击展开
    fireEvent.click(groupHeader);
    expect(screen.getByText("可折叠会话")).toBeTruthy();
  });

  it("switches grouping between workstream and project", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "切换测试会话",
        owner_workstream_id: "ws-1",
        project_id: "p-1",
      }),
    ]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "按任务分组名" }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "p-1", name: "按项目分组名" }),
    ]);

    renderSidebar();

    expect(await screen.findByText("按任务分组名")).toBeTruthy();
    expect(screen.queryByText("按项目分组名")).toBeNull();

    // 切换至按项目归类
    const projectToggle = screen.getByRole("radio", { name: "按项目归类" });
    fireEvent.click(projectToggle);

    const projectGroupHeader = await screen.findByRole("button", { name: /按项目分组名/ });
    expect(screen.queryByText("按任务分组名")).toBeNull();

    // 收起项目分组
    fireEvent.click(projectGroupHeader);
    expect(screen.queryByText("切换测试会话")).toBeNull();

    // 切回按任务归类
    fireEvent.click(screen.getByRole("radio", { name: "按任务归类" }));

    expect(await screen.findByText("按任务分组名")).toBeTruthy();
  });

  it("handles unassigned workstream and unassigned project groups", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-orphan",
        title: "孤立会话",
        owner_workstream_id: null,
        project_id: null,
      }),
    ]);

    renderSidebar();

    // 任务维度：未归属任务
    expect(await screen.findByText("未归属任务")).toBeTruthy();

    // 项目维度：未归属项目
    const projectToggle = screen.getByRole("radio", { name: "按项目归类" });
    fireEvent.click(projectToggle);

    expect(await screen.findByText("未归属项目")).toBeTruthy();
  });

  it("refetches recent sessions on EVT_SYNCED event", async () => {
    renderSidebar();
    await waitFor(() => expect(api.listSessions).toHaveBeenCalledTimes(1));

    window.dispatchEvent(new CustomEvent(EVT_SYNCED));
    await waitFor(() => expect(api.listSessions).toHaveBeenCalledTimes(2));
  });

  it("switches grouping to agent and collapses each agent independently", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "第一条会话",
        agent: "claude_code",
      }),
      session({
        id: "s-2",
        title: "第二条会话",
        agent: "codex",
      }),
    ]);

    renderSidebar();

    const agentToggle = screen.getByRole("radio", { name: "按代理归类" });

    fireEvent.click(agentToggle);

    expect(await screen.findByRole("button", { name: /Claude Code/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Codex/ })).toBeTruthy();
    expect(screen.getByText("第一条会话")).toBeTruthy();
    expect(screen.getByText("第二条会话")).toBeTruthy();

    const claudeGroupHeader = screen.getByRole("button", { name: /Claude Code/ });
    fireEvent.click(claudeGroupHeader);
    expect(screen.queryByText("第一条会话")).toBeNull();
    expect(screen.getByText("第二条会话")).toBeTruthy();
  });

  it("determines recent sessions strictly by last_conversation_at, ignoring last_activity_at", async () => {
    const twoDaysAgo = new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString();
    const tenDaysAgo = new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString();

    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-claude-recent-conv",
        title: "近7天有新消息的会话",
        agent: "claude_code",
        last_conversation_at: twoDaysAgo,
      }),
      session({
        id: "s-claude-old-conv",
        title: "仅有文件活动但无新消息的会话",
        agent: "claude_code",
        last_conversation_at: tenDaysAgo,
        last_activity_at: twoDaysAgo,
      }),
      session({
        id: "s-claude-no-conv",
        title: "从未有对话的会话",
        agent: "claude_code",
        last_conversation_at: null,
        last_activity_at: twoDaysAgo,
      }),
    ]);

    renderSidebar();

    const agentToggle = screen.getByRole("radio", { name: "按代理归类" });
    fireEvent.click(agentToggle);

    expect(await screen.findByRole("button", { name: /Claude Code/ })).toBeTruthy();
    expect(screen.getByText("近7天有新消息的会话")).toBeTruthy();
    expect(screen.queryByText("仅有文件活动但无新消息的会话")).toBeNull();
    expect(screen.queryByText("从未有对话的会话")).toBeNull();
  });
});
