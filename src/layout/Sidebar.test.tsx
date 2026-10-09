// Sidebar contract: grouped sessions, retained terminals and exact native identity.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
import { api } from "../api";
import { viewState } from "../hooks/useViewState";
import { sessionPages, sessionTerminals, rememberSessionPage } from "../features/sessions/sessionNavigation";
import { EVT_TERMINALS, EVT_SYNCED } from "../app/routes";
import type { Route } from "../app/routes";
import type { TerminalSummary, Session, WorkstreamCardData, ProjectCardData } from "../types";

vi.mock("../api", () => ({
  api: {
    terminalList: vi.fn(),
    listSessions: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listProjectCards: vi.fn(),
  },
}));

beforeEach(() => {
  viewState.clear();
  sessionPages.clear();
  sessionTerminals.clear();
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
  vi.mocked(api.listSessions).mockReset().mockResolvedValue([]);
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

function renderSidebar(route: Route = { view: "new-session" }, navigate = vi.fn()) {
  render(<Sidebar route={route} navigate={navigate} />);
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

  it("opens the sessions board from the session sidebar", async () => {
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByRole("button", { name: "会话看板" }));
    expect(navigate).toHaveBeenCalledWith({ view: "sessions" });
  });
});

describe("Sidebar unified sessions", () => {
  it("merges multiple terminals with their recent session into one navigable row", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "共享会话" })]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t1", session_id: "s1", session_title: "共享会话" }),
      terminal({ terminal_id: "t2", session_id: "s1", session_title: "共享会话", live: false }),
    ]);
    const navigate = renderSidebar({ view: "terminal", terminalId: "t2" });
    await screen.findByLabelText("1 个运行中终端，1 个已退出终端");
    expect(screen.getAllByText("共享会话")).toHaveLength(1);
    expect(screen.getByText("共享会话").closest("button")?.className).toContain("active");
    fireEvent.click(screen.getByText("共享会话"));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t1" }));
  });

  it("keeps old and archived sessions visible while any retained terminal belongs to them", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "旧会话", archived_at: "archived", last_conversation_at: "2020-01-01T00:00:00Z" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "s1", live: false })]);
    renderSidebar();
    await screen.findByLabelText("0 个运行中终端，1 个已退出终端");
    expect(screen.getByText("旧会话")).toBeTruthy();
    expect(api.listSessions).toHaveBeenCalledWith(undefined, undefined, "all");
  });

  it("keeps unbound terminals accessible and merges them once identity is known", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "已绑定会话" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ live: false })]);
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByTitle("新会话 · /repo/x"));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-1" });
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "s1", session_title: "已绑定会话", identity_revision: 1 })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await waitFor(() => expect(screen.queryByTitle("新会话 · /repo/x")).toBeNull());
    expect(screen.getAllByText("已绑定会话")).toHaveLength(1);
  });

  it("restores the session subpage, but does not reopen a terminal that switched identity", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1" })]);
    rememberSessionPage("s1", { view: "session", sessionId: "s1", entry: "conversation" });
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByText("修复侧栏问题"));
    expect(navigate).toHaveBeenLastCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
    rememberSessionPage("s1", { view: "terminal", terminalId: "moved" });
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "moved", session_id: "s2", session_title: "另一会话" })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await screen.findByText("另一会话");
    fireEvent.click(screen.getByText("修复侧栏问题"));
    expect(navigate).toHaveBeenLastCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
  });

  it("ignores an old terminal list response after a newer identity refresh", async () => {
    let resolveOld!: (rows: TerminalSummary[]) => void;
    vi.mocked(api.terminalList).mockReturnValueOnce(new Promise(resolve => { resolveOld = resolve; }))
      .mockResolvedValueOnce([terminal({ session_id: "b", session_title: "会话 B", identity_revision: 2 })]);
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(1));
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await screen.findByText("会话 B");
    await act(async () => { resolveOld([terminal({ session_id: "a", session_title: "旧会话 A", identity_revision: 1 })]); });
    expect(screen.queryByText("旧会话 A")).toBeNull();
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
    expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s-1", entry: "conversation" });
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
