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
    getAgentStatus: vi.fn(), continueSessionDesktop: vi.fn(), terminalForSession: vi.fn(), launchEmbeddedResume: vi.fn(),
    terminalList: vi.fn(),
    listSessions: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listProjectCards: vi.fn(),
  },
}));

beforeEach(() => {
  vi.mocked(api.getAgentStatus).mockReset().mockResolvedValue({});
  vi.mocked(api.continueSessionDesktop).mockReset().mockResolvedValue({ note: "opened" } as never);
  vi.mocked(api.terminalForSession).mockReset().mockResolvedValue(null);
  vi.mocked(api.launchEmbeddedResume).mockReset().mockResolvedValue({ terminal_id: "spawned" } as never);
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
    owner_workstream_id: null, identity_revision: 0,
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
    const newSession = screen.getByRole("button", { name: "新建" });
    expect(newSession.className).toContain("active");
    fireEvent.click(newSession);
    expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
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
    await screen.findByRole("button", { name: "终端 · 运行中：共享会话" });
    expect(screen.getAllByText("共享会话")).toHaveLength(1);
    expect(screen.getByText("共享会话").closest(".sidebar-recent-item")?.className).toContain("active");
    fireEvent.click(screen.getByText("共享会话"));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t1" }));
  });

  it("keeps old and archived sessions visible while any retained terminal belongs to them", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "旧会话", archived_at: "archived", last_conversation_at: "2020-01-01T00:00:00Z" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "s1", live: false })]);
    renderSidebar();
    await screen.findByRole("button", { name: "终端 · 已退出：旧会话" });
    expect(screen.getByText("旧会话")).toBeTruthy();
    expect(screen.getByRole("img", { name: "0 个运行中终端，1 个已退出终端" })).toBeTruthy();
    expect(api.listSessions).toHaveBeenCalledWith(undefined, undefined, "all");
  });

  it("keeps unbound terminals accessible and merges them once identity is known", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "已绑定会话" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ live: false })]);
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByTitle("新会话 · /repo/x"));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t-1", initialAgent: "codex" }));
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
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "session", sessionId: "s1", entry: "conversation", initialTitle: "修复侧栏问题", initialAgent: "codex" }));
    rememberSessionPage("s1", { view: "terminal", terminalId: "moved" });
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "moved", session_id: "s2", session_title: "另一会话" })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await screen.findByText("另一会话");
    fireEvent.click(screen.getByText("修复侧栏问题"));
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "session", sessionId: "s1", entry: "conversation", initialTitle: "修复侧栏问题", initialAgent: "codex" }));
  });

  it("sidebar prefers terminal capability and reuses the selected instance independently of aggregate status", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "共享会话" }), session({ id: "desktop", title: "桌面会话", agent: "workbuddy" })]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "live", session_id: "s1" }),
      terminal({ terminal_id: "exited", session_id: "s1", live: false }),
    ]);
    sessionTerminals.set("s1", "exited");
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByRole("button", { name: "终端 · 已退出：共享会话" }));
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ terminalId: "exited", initialSessionId: "s1", initialTitle: "共享会话" }));
    expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "在桌面应用中继续：共享会话" })).toBeNull();
    expect(screen.getByRole("img", { name: "1 个运行中终端，1 个已退出终端" })).toBeTruthy();
    navigate.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "在桌面应用中继续：桌面会话" }));
    await waitFor(() => expect(api.continueSessionDesktop).toHaveBeenCalledWith("desktop"));
    expect(navigate).not.toHaveBeenCalled();
  });

  it("launches explicitly from the sidebar only when no terminal exists and respects archive/format availability", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s1", title: "待继续", agent: "pi" }),
      session({ id: "s2", title: "桌面会话", agent: "workbuddy" }),
      session({ id: "s3", title: "已归档", archived_at: "archived" }),
    ]);
    const navigate = renderSidebar({ view: "session", sessionId: "s3" });
    fireEvent.click(await screen.findByRole("button", { name: "在终端中继续：待继续" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "spawned", initialTitle: "待继续", initialAgent: "pi" })));
    expect(api.launchEmbeddedResume).toHaveBeenCalledWith("s1");
    expect(screen.queryByRole("button", { name: "在桌面应用中继续：待继续" })).toBeNull();
    expect(screen.queryByRole("button", { name: "在终端中继续：桌面会话" })).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "在终端中继续：已归档" }).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "在桌面应用中继续：已归档" })).toBeNull();
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
  it("renders empty state when there are no sessions", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([]);

    renderSidebar();
    expect(await screen.findByText("暂无会话")).toBeTruthy();
  });

  it("displays 工作区 title and supports filtering by 最近5天, 未归档, 可终端继续", async () => {
    const activeRecent = session({
      id: "s-recent",
      title: "近期活跃会话",
      agent: "codex",
      last_conversation_at: new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString(),
    });
    const oldSession = session({
      id: "s-old",
      title: "较旧会话",
      agent: "codex",
      last_conversation_at: new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString(),
    });
    const archivedSession = session({
      id: "s-archived",
      title: "已归档会话",
      agent: "codex",
      last_conversation_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
      archived_at: new Date().toISOString(),
    });
    const desktopOnlySession = session({
      id: "s-desktop",
      title: "桌面代理会话",
      agent: "qoder",
      last_conversation_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
    });

    vi.mocked(api.listSessions).mockResolvedValue([activeRecent, oldSession, archivedSession, desktopOnlySession]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "测试任务" }),
    ]);

    renderSidebar();

    // 1. Header shows "工作区"
    expect(await screen.findByText("工作区")).toBeTruthy();

    // 2. Default: shows all sessions (not restricted to 7 days)
    expect(screen.getByText("近期活跃会话")).toBeTruthy();
    expect(screen.getByText("较旧会话")).toBeTruthy();
    expect(screen.getByText("已归档会话")).toBeTruthy();
    expect(screen.getByText("桌面代理会话")).toBeTruthy();

    // 3. Open filter menu
    const filterBtn = screen.getByRole("button", { name: "筛选会话" });
    fireEvent.click(filterBtn);
    expect(screen.getByRole("menu")).toBeTruthy();

    // 4. Filter by 最近5天
    const filterRecentBtn = screen.getByRole("menuitemcheckbox", { name: "最近5天" });
    fireEvent.click(filterRecentBtn);
    expect(screen.getByText("近期活跃会话")).toBeTruthy();
    expect(screen.queryByText("较旧会话")).toBeNull();

    // 5. Multi-select: Filter by 未归档
    const filterUnarchivedBtn = screen.getByRole("menuitemcheckbox", { name: "未归档" });
    fireEvent.click(filterUnarchivedBtn);
    expect(screen.getByText("近期活跃会话")).toBeTruthy();
    expect(screen.queryByText("已归档会话")).toBeNull();

    // 6. Multi-select: Filter by 可终端继续
    const filterCanContinueBtn = screen.getByRole("menuitemcheckbox", { name: "可终端继续" });
    fireEvent.click(filterCanContinueBtn);
    expect(screen.getByText("近期活跃会话")).toBeTruthy();
    expect(screen.queryByText("桌面代理会话")).toBeNull();
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
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s-1", entry: "conversation", initialTitle: "修复侧栏问题", initialAgent: "codex" }));
  });

  it("navigates to session when clicking the row edge/container", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-edge",
        title: "边缘点击会话",
        last_conversation_at: new Date(Date.now() - 1000).toISOString(),
      }),
    ]);
    const navigate = renderSidebar();
    const rowTitle = await screen.findByText("边缘点击会话");
    const rowContainer = rowTitle.closest(".sidebar-recent-item")!;
    fireEvent.click(rowContainer);
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s-edge" }));
  });

  it("enables marquee scrolling on hover when title overflows", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-long",
        title: "这是一个特别长特别长的超出会话标题用于测试文字滚动效果",
        last_conversation_at: new Date(Date.now() - 1000).toISOString(),
      }),
    ]);
    renderSidebar();
    const rowTitle = await screen.findByText("这是一个特别长特别长的超出会话标题用于测试文字滚动效果");
    const titleContainer = rowTitle.closest(".sidebar-session-title") as HTMLElement;
    const rowContainer = rowTitle.closest(".sidebar-recent-item")!;

    Object.defineProperty(titleContainer, "scrollWidth", { configurable: true, value: 300 });
    Object.defineProperty(titleContainer, "clientWidth", { configurable: true, value: 150 });

    fireEvent.mouseEnter(rowContainer);
    expect(titleContainer.className).toContain("scrolling");
    expect(titleContainer.style.getPropertyValue("--overflow-width")).toBe("186px");

    fireEvent.mouseLeave(rowContainer);
    expect(titleContainer.className).not.toContain("scrolling");
  });

  it("keeps an unbound terminal in its selected task and retains that group after ingestion", async () => {
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([workstreamCard({ title: "启动任务" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ owner_workstream_id: "ws-1" })]);
    const navigate = renderSidebar();
    const group = await screen.findByRole("button", { name: /启动任务/ });
    expect(screen.getByTitle("新会话 · /repo/x")).toBeTruthy();
    expect(screen.queryByText("未归属任务")).toBeNull();
    fireEvent.click(group);
    expect(screen.queryByTitle("新会话 · /repo/x")).toBeNull();
    fireEvent.click(group);
    fireEvent.click(screen.getByTitle("新会话 · /repo/x"));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "t-1" }));
    vi.mocked(api.listSessions).mockResolvedValue([session({ title: "摄入会话" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "s-1", owner_workstream_id: "ws-1" })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); window.dispatchEvent(new CustomEvent(EVT_SYNCED)); });
    await waitFor(() => expect(screen.queryByTitle("新会话 · /repo/x")).toBeNull());
    await screen.findByText("摄入会话");
    expect(screen.queryByTitle("新会话 · /repo/x")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /启动任务/ }));
    expect(screen.queryByText("摄入会话")).toBeNull();
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

  it("determines recent sessions strictly by last_conversation_at when filtering by recent 5 days", async () => {
    const twoDaysAgo = new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString();
    const tenDaysAgo = new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString();

    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-claude-recent-conv",
        title: "近5天有新消息的会话",
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

    const filterBtn = screen.getByRole("button", { name: "筛选会话" });
    fireEvent.click(filterBtn);
    const filterRecentBtn = screen.getByRole("menuitemcheckbox", { name: "最近5天" });
    fireEvent.click(filterRecentBtn);

    const agentToggle = screen.getByRole("radio", { name: "按代理归类" });
    fireEvent.click(agentToggle);

    expect(await screen.findByRole("button", { name: /Claude Code/ })).toBeTruthy();
    expect(screen.getByText("近5天有新消息的会话")).toBeTruthy();
    expect(screen.queryByText("仅有文件活动但无新消息的会话")).toBeNull();
    expect(screen.queryByText("从未有对话的会话")).toBeNull();
  });

  it("sorts sessions within a group by started_at / created_at desc", async () => {
    const olderCreated = session({
      id: "s-older-created",
      title: "更早创建但最近聊天的会话",
      agent: "codex",
      owner_workstream_id: "ws-1",
      started_at: "2026-10-01T10:00:00.000Z",
      last_conversation_at: "2026-10-10T12:00:00.000Z",
    });
    const newerCreated = session({
      id: "s-newer-created",
      title: "更晚创建但较早聊天的会话",
      agent: "codex",
      owner_workstream_id: "ws-1",
      started_at: "2026-10-05T10:00:00.000Z",
      last_conversation_at: "2026-10-06T12:00:00.000Z",
    });

    vi.mocked(api.listSessions).mockResolvedValue([olderCreated, newerCreated]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      workstreamCard({ id: "ws-1", title: "测试排序任务" }),
    ]);

    renderSidebar();
    await screen.findByText("测试排序任务");

    const links = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-session-link"));
    expect(links[0].textContent).toContain("更晚创建但较早聊天的会话");
    expect(links[1].textContent).toContain("更早创建但最近聊天的会话");
  });

  it("sorts project and workstream groups by created_at desc", async () => {
    const wsOld = workstreamCard({ id: "ws-old", title: "旧任务", created_at: "2026-09-01T00:00:00Z" });
    const wsNew = workstreamCard({ id: "ws-new", title: "新任务", created_at: "2026-10-01T00:00:00Z" });

    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-ws-old", title: "旧任务会话", owner_workstream_id: "ws-old" }),
      session({ id: "s-ws-new", title: "新任务会话", owner_workstream_id: "ws-new" }),
    ]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([wsOld, wsNew]);

    renderSidebar();
    await screen.findByText("新任务");

    const groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    expect(groupHeaders[0].textContent).toContain("新任务");
    expect(groupHeaders[1].textContent).toContain("旧任务");
  });

  it("sorts agent groups by terminal capability first, then by name", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-qoder", title: "Qoder会话", agent: "qoder" }),
      session({ id: "s-dsh", title: "DSH会话", agent: "dsh" }),
      session({ id: "s-claude", title: "Claude会话", agent: "claude_code" }),
      session({ id: "s-codex", title: "Codex会话", agent: "codex" }),
    ]);

    renderSidebar();
    const agentRadio = screen.getByRole("radio", { name: "按代理归类" });
    fireEvent.click(agentRadio);

    await screen.findByRole("button", { name: /Claude Code/ });
    const groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    const titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("Claude Code");
    expect(titles[1]).toContain("Codex");
    expect(titles[2]).toContain("DSH");
    expect(titles[3]).toContain("Qoder");
  });

  it("differentiates running vs exited terminal continue buttons with status classes", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-live", title: "运行中会话", agent: "codex" }),
      session({ id: "s-exited", title: "已退出会话", agent: "codex" }),
    ]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-live", session_id: "s-live", live: true }),
      terminal({ terminal_id: "t-exited", session_id: "s-exited", live: false }),
    ]);

    renderSidebar({ view: "terminal", terminalId: "t-live" });

    const liveBtn = await screen.findByRole("button", { name: /终端 · 运行中：运行中会话/ });
    expect(liveBtn.className).toContain("terminal-running");
    expect(liveBtn.className).toContain("active");

    const exitedBtn = await screen.findByRole("button", { name: /终端 · 已退出：已退出会话/ });
    expect(exitedBtn.className).toContain("terminal-exited");
    expect(exitedBtn.className).not.toContain("active");

    const liveRow = liveBtn.closest(".sidebar-recent-item")!;
    const liveDot = liveRow.querySelector(".sidebar-session-link .sidebar-session-dot");
    expect(liveDot).toBeTruthy();
    expect(liveDot?.className).toContain("running");
    const liveTitle = liveRow.querySelector(".sidebar-session-title")!;
    expect(liveTitle.compareDocumentPosition(liveDot!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    const exitedRow = exitedBtn.closest(".sidebar-recent-item")!;
    const exitedDot = exitedRow.querySelector(".sidebar-session-link .sidebar-session-dot");
    expect(exitedDot).toBeTruthy();
    expect(exitedDot?.className).toContain("exited");
  });
});
