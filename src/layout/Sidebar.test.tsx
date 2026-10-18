import { invalidateTargets, removedTargets } from "../app/removedTargets";
// Sidebar contract: grouped sessions, retained terminals and exact native identity.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
import { api } from "../api";
import { viewState } from "../hooks/useViewState";
import { sessionPages, sessionTerminals, rememberSessionPage } from "../features/sessions/sessionNavigation";
import { EVT_TERMINALS, EVT_SYNCED, EVT_TERMINAL_ACTIVITY } from "../app/routes";
import type { Route } from "../app/routes";
import type { TerminalSummary, Session, ProjectCardData } from "../types";

vi.mock("../api", () => ({
  api: {
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    getAgentStatus: vi.fn(), continueSessionDesktop: vi.fn(), terminalForSession: vi.fn(), launchEmbeddedResume: vi.fn(),
    terminalList: vi.fn(), terminalReconnect: vi.fn(),
    listSessions: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listProjectCards: vi.fn(),
  },
}));

beforeEach(() => {
  Object.values(removedTargets).forEach(ids => ids.clear());
  vi.mocked(api.getAgentStatus).mockReset().mockResolvedValue({});
  vi.mocked(api.continueSessionDesktop).mockReset().mockResolvedValue({ note: "opened" } as never);
  vi.mocked(api.terminalForSession).mockReset().mockResolvedValue(null);
  vi.mocked(api.terminalReconnect).mockReset().mockResolvedValue({ terminal_id: "reconnected" } as never);
  vi.mocked(api.launchEmbeddedResume).mockReset().mockResolvedValue({ terminal_id: "spawned" } as never);
  viewState.clear();
  sessionPages.clear();
  sessionTerminals.clear();
  localStorage.clear();
  vi.mocked(api.getSessionTerminalStates).mockImplementation(async ids => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }])));
  vi.mocked(api.getSessionTerminalStates).mockClear();
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
  vi.mocked(api.listSessions).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
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
    initial_workstream_ids: [], identity_revision: 0,
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
    tracked_workstream_ids: [],
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

function projectCard(over: Partial<ProjectCardData> = {}): ProjectCardData {
  return {
    id: "p-1",
    name: "项目A",
    name_customized: false, visibility: "normal",
    kind: "single_directory" as const,
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

function switchGroupBy(name: "按项目归类" | "按 Agent 归类" | "不分组（全部会话）") {
  const trigger = screen.getByRole("button", { name: /会话归类/ });
  fireEvent.click(trigger);
  const item = screen.getByRole("menuitemradio", { name });
  fireEvent.click(item);
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
    await screen.findByRole("button", { name: "终端 · 已连接：共享会话" });
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
    expect(screen.getByRole("img", { name: "已退出" })).toBeTruthy();
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
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "session", sessionId: "s1", initialTitle: "修复侧栏问题", initialAgent: "codex" }));
  });

  it("sidebar prefers terminal capability and reuses the selected instance independently of aggregate status", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "共享会话" }), session({ id: "desktop", title: "桌面会话", agent: "workbuddy" })]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "live", session_id: "s1" }),
      terminal({ terminal_id: "exited", session_id: "s1", live: false }),
    ]);
    sessionTerminals.set("s1", "exited");
    const navigate = renderSidebar();
    fireEvent.click(await screen.findByRole("button", { name: "终端 · 已连接：共享会话" }));
    await waitFor(() => expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ terminalId: "live", initialSessionId: "s1", initialTitle: "共享会话" })));
    expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "在桌面应用中继续：共享会话" })).toBeNull();
    expect(screen.getByRole("img", { name: "已退出" })).toBeTruthy();
    navigate.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "在桌面应用中继续：桌面会话" }));
    await waitFor(() => expect(api.continueSessionDesktop).toHaveBeenCalledWith("desktop"));
    expect(navigate).not.toHaveBeenCalled();
  });

  it("reconnects an exited terminal from its sidebar action instead of opening a dead input", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "已退出" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "exited", session_id: "s1", live: false })]);
    const navigate = renderSidebar();
    const button = await screen.findByRole<HTMLButtonElement>("button", { name: "终端 · 已退出：已退出" });
    await waitFor(() => expect(button.disabled).toBe(false));
    fireEvent.click(button);
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "reconnected", initialSessionId: "s1" })));
    expect(api.terminalReconnect).toHaveBeenCalledWith("exited", "s1");
    expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  });

  it("uses the same backend refusal for the terminal button and continuable filter", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "ready", title: "可以继续" }), session({ id: "blocked", title: "CLI 缺失" })]);
    vi.mocked(api.getSessionTerminalStates).mockResolvedValue({
      ready: { available: true, reason: null }, blocked: { available: false, reason: "未检测到 CLI" },
    });
    renderSidebar();
    const button = await screen.findByRole<HTMLButtonElement>("button", { name: "在终端中继续：CLI 缺失" });
    await waitFor(() => expect(button.title).toBe("未检测到 CLI"));
    expect(button.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "筛选与排序会话" }));
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "可终端继续" }));
    expect(screen.queryByText("CLI 缺失")).toBeNull();
    expect(screen.getByText("可以继续")).toBeTruthy();
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
  it("renders empty state and refetches recent sessions on EVT_SYNCED", async () => {
    vi.mocked(api.listSessions).mockResolvedValueOnce([]);
    renderSidebar();
    expect(await screen.findByText("暂无会话")).toBeTruthy();

    vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s-synced", title: "同步来的会话" })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_SYNCED)); });
    expect(await screen.findByText("同步来的会话")).toBeTruthy();
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
      title: "桌面 Agent 会话",
      agent: "qoder",
      last_conversation_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
    });

    vi.mocked(api.listSessions).mockResolvedValue([activeRecent, oldSession, archivedSession, desktopOnlySession]);
    vi.mocked(api.getSessionTerminalStates).mockResolvedValue({
      "s-recent": { available: true, reason: null }, "s-old": { available: true, reason: null },
      "s-archived": { available: false, reason: "已归档" }, "s-desktop": { available: false, reason: "仅支持桌面端" },
    });
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "测试任务" }),
    ]);

    renderSidebar();

    // 1. Header shows group trigger button replacing static title
    expect(await screen.findByRole("button", { name: /会话归类/ })).toBeTruthy();
    expect(screen.getByText("项目")).toBeTruthy();

    // 2. Default: shows all sessions (not restricted to 7 days)
    expect(screen.getByText("近期活跃会话")).toBeTruthy();
    expect(screen.getByText("较旧会话")).toBeTruthy();
    expect(screen.getByText("已归档会话")).toBeTruthy();
    expect(screen.getByText("桌面 Agent 会话")).toBeTruthy();

    // 3. Open filter menu
    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
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
    expect(screen.queryByText("桌面 Agent 会话")).toBeNull();
  });

  it.each([true, false])("retains open terminals (live=%s), filters the current history page, and reapplies filters after removal", async live => {
    const oldSessionWithTerminal = session({
      id: "s-term",
      title: "有终端启动的会话",
      agent: "codex",
      last_conversation_at: new Date(Date.now() - 15 * 24 * 60 * 60 * 1000).toISOString(),
      archived_at: new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString(),
    });
    const oldSessionWithoutTerminal = session({
      id: "s-plain",
      title: "普通较旧已归档会话",
      agent: "codex",
      last_conversation_at: new Date(Date.now() - 15 * 24 * 60 * 60 * 1000).toISOString(),
      archived_at: new Date(Date.now() - 10 * 24 * 60 * 60 * 1000).toISOString(),
    });

    vi.mocked(api.listSessions).mockResolvedValue([oldSessionWithTerminal, oldSessionWithoutTerminal]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-1", session_id: "s-term", session_title: "有终端启动的会话", live }),
    ]);

    renderSidebar({ view: "session", sessionId: "s-plain" });

    expect(await screen.findByText("有终端启动的会话")).toBeTruthy();
    expect(screen.getByText("普通较旧已归档会话")).toBeTruthy();

    // Open filter menu and enable both "最近5天" and "未归档"
    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
    fireEvent.click(filterBtn);

    const filterRecentBtn = screen.getByRole("menuitemcheckbox", { name: "最近5天" });
    fireEvent.click(filterRecentBtn);

    const filterUnarchivedBtn = screen.getByRole("menuitemcheckbox", { name: "未归档" });
    fireEvent.click(filterUnarchivedBtn);

    // Plain session is filtered out because it is older than 5 days and archived
    expect(screen.queryByText("普通较旧已归档会话")).toBeNull();

    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "可终端继续" }));

    // Session with a started terminal bypasses filter restrictions and remains visible
    expect(screen.getByText("有终端启动的会话")).toBeTruthy();
    vi.mocked(api.terminalList).mockResolvedValue([]);
    act(() => window.dispatchEvent(new CustomEvent(EVT_TERMINALS)));
    await waitFor(() => expect(screen.queryByText("有终端启动的会话")).toBeNull());
  });

  it.each(["project", "none"])("filters only unpinned sessions with live terminals when grouped by %s", async (groupBy) => {
    viewState.set("sidebar.recent.groupBy.v2", groupBy);
    localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["pinned"]));
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "live", title: "仍有终端运行" }),
      session({ id: "exited", title: "当前已退出会话", project_id: "p-exited", agent: "pi" }),
      session({ id: "idle", title: "未启动终端" }),
      session({ id: "pinned", title: "置顶但未运行" }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard(), projectCard({ id: "p-exited", name: "已退出项目" }),
    ]);
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-old", session_id: "live", live: false }),
      terminal({ terminal_id: "t-live", session_id: "live" }),
      terminal({ terminal_id: "t-exited", session_id: "exited", live: false }),
      terminal({ terminal_id: "t-unbound", session_title: "待绑定终端" }),
    ]);
    renderSidebar({ view: "session", sessionId: "exited" });
    await screen.findByText("待绑定终端");
    await screen.findByText("置顶但未运行");
    expect(screen.getByText("当前已退出会话")).toBeTruthy();
    expect(screen.getByText("未启动终端")).toBeTruthy();

    const toggle = screen.getByRole("button", { name: "仅显示运行中会话" });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getAllByText("仍有终端运行")).toHaveLength(1);
    expect(screen.getByText("待绑定终端")).toBeTruthy();
    expect(screen.getByText("置顶但未运行")).toBeTruthy();
    expect(screen.queryByText("当前已退出会话")).toBeNull();
    expect(screen.queryByText("未启动终端")).toBeNull();
    expect(screen.queryByTitle("已退出项目 (1)")).toBeNull();

    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("当前已退出会话")).toBeTruthy();
    expect(screen.getByText("未启动终端")).toBeTruthy();
  });

  it("refreshes the running filter on terminal start and exit without hiding pinned sessions", async () => {
    localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["pinned"]));
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "live", title: "运行状态变化" }),
      session({ id: "pinned", title: "始终保留的置顶" }),
    ]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "live" })]);
    renderSidebar();
    await screen.findByText("运行状态变化");
    await screen.findByText("始终保留的置顶");

    fireEvent.click(screen.getByRole("button", { name: "仅显示运行中会话" }));
    fireEvent.click(screen.getByRole("button", { name: "筛选与排序会话" }));
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "未归档" }));
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "live", live: false })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await waitFor(() => expect(screen.queryByText("运行状态变化")).toBeNull());
    expect(screen.getByText("暂无运行中会话")).toBeTruthy();
    expect(screen.getByText("始终保留的置顶")).toBeTruthy();

    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "live" })]);
    act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
    await screen.findByText("运行状态变化");
    expect(screen.queryByText("暂无运行中会话")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "仅显示运行中会话" }));
    expect(screen.getByRole("menuitemcheckbox", { name: "未归档" }).getAttribute("aria-checked")).toBe("true");
  });

  it("groups by project by default and navigates to session detail on click", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "修复侧栏问题",
        project_id: "ws-1",
        last_conversation_at: new Date(Date.now() - 1000).toISOString(),
      }),
      session({
        id: "s-edge",
        title: "边缘点击会话",
        project_id: "ws-1",
        last_conversation_at: new Date(Date.now() - 1000).toISOString(),
      }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "侧栏任务" }),
    ]);

    const navigate = renderSidebar();
    expect(await screen.findByText("侧栏任务")).toBeTruthy();

    // Click on title text
    fireEvent.click(screen.getByText("修复侧栏问题"));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s-1", initialTitle: "修复侧栏问题", initialAgent: "codex" }));

    // Click on row container edge
    const edgeRow = screen.getByText("边缘点击会话").closest(".sidebar-recent-item")!;
    fireEvent.click(edgeRow);
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s-edge" }));
  });

  it("toggles group collapse when group header is clicked", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-1",
        title: "可折叠会话",
        project_id: "ws-1",
      }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "折叠测试任务" }),
    ]);

    renderSidebar();
    const groupHeader = await screen.findByRole("button", { name: /折叠测试任务/ });
    expect(screen.getByText("可折叠会话")).toBeTruthy();

    // 点击收起
    fireEvent.click(groupHeader);
    expect(screen.queryByText("可折叠会话")).toBeNull();

    cleanup();
    viewState.clear();
    renderSidebar();
    const restoredHeader = await screen.findByRole("button", { name: /折叠测试任务/ });
    expect(restoredHeader.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("可折叠会话")).toBeNull();

    fireEvent.click(restoredHeader);
    expect(screen.getByText("可折叠会话")).toBeTruthy();
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

    switchGroupBy("按 Agent 归类");

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

    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
    fireEvent.click(filterBtn);
    const filterRecentBtn = screen.getByRole("menuitemcheckbox", { name: "最近5天" });
    fireEvent.click(filterRecentBtn);

    switchGroupBy("按 Agent 归类");

    expect(await screen.findByRole("button", { name: /Claude Code/ })).toBeTruthy();
    expect(screen.getByText("近5天有新消息的会话")).toBeTruthy();
    expect(screen.queryByText("仅有文件活动但无新消息的会话")).toBeNull();
    expect(screen.queryByText("从未有对话的会话")).toBeNull();
  });

  it("sorts agent groups by terminal capability first, then by name", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-qoder", title: "Qoder会话", agent: "qoder" }),
      session({ id: "s-dsh", title: "DSH会话", agent: "dsh" }),
      session({ id: "s-claude", title: "Claude会话", agent: "claude_code" }),
      session({ id: "s-codex", title: "Codex会话", agent: "codex" }),
    ]);

    renderSidebar();
    switchGroupBy("按 Agent 归类");

    await screen.findByRole("button", { name: /Claude Code/ });
    const groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    const titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("Claude Code");
    expect(titles[1]).toContain("Codex");
    expect(titles[2]).toContain("DSH");
    expect(titles[3]).toContain("Qoder");
  });

  it("sorts agent groups dynamically by createdAt or updatedAt", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({
        id: "s-codex",
        title: "Codex 会话",
        agent: "codex",
        started_at: "2026-10-01T10:00:00.000Z",
        last_conversation_at: "2026-10-10T12:00:00.000Z",
      }),
      session({
        id: "s-claude",
        title: "Claude 会话",
        agent: "claude_code",
        started_at: "2026-10-05T10:00:00.000Z",
        last_conversation_at: "2026-10-06T12:00:00.000Z",
      }),
    ]);

    renderSidebar();
    switchGroupBy("按 Agent 归类");

    await screen.findByRole("button", { name: /Claude Code/ });

    // Default sortBy is createdAt: claude_code was created 2026-10-05, codex 2026-10-01
    let groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    let titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("Claude Code");
    expect(titles[1]).toContain("Codex");

    // Switch to 最近更新
    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
    fireEvent.click(filterBtn);
    fireEvent.click(screen.getByRole("menuitemradio", { name: "最近更新" }));

    // Under updatedAt: codex was updated 2026-10-10, claude 2026-10-06
    groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("Codex");
    expect(titles[1]).toContain("Claude Code");
  });

  it("sorts project groups dynamically by createdAt or updatedAt", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "p-a", name: "项目甲", created_at: "2026-10-01T10:00:00.000Z", updated_at: "2026-10-10T12:00:00.000Z", last_activity_at: "2026-10-10T12:00:00.000Z" }),
      projectCard({ id: "p-b", name: "项目乙", created_at: "2026-10-05T10:00:00.000Z", updated_at: "2026-10-06T12:00:00.000Z", last_activity_at: "2026-10-06T12:00:00.000Z" }),
    ]);
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-a", project_id: "p-a", title: "会话甲", started_at: "2026-10-01T10:00:00.000Z", last_conversation_at: "2026-10-10T12:00:00.000Z" }),
      session({ id: "s-b", project_id: "p-b", title: "会话乙", started_at: "2026-10-05T10:00:00.000Z", last_conversation_at: "2026-10-06T12:00:00.000Z" }),
    ]);

    renderSidebar();
    await screen.findByText("项目甲");

    // Default sortBy is createdAt: Project B was created 2026-10-05, Project A 2026-10-01
    let groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    let titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("项目乙");
    expect(titles[1]).toContain("项目甲");

    // Switch to 最近更新
    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
    fireEvent.click(filterBtn);
    fireEvent.click(screen.getByRole("menuitemradio", { name: "最近更新" }));

    // Under updatedAt: Project A was updated 2026-10-10, Project B 2026-10-06
    groupHeaders = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-recent-group-header"));
    titles = groupHeaders.map(h => h.textContent);
    expect(titles[0]).toContain("项目甲");
    expect(titles[1]).toContain("项目乙");
  });

  it("moves pinned sessions out of the regular list and restores them on unpin", async () => {
    viewState.set("sidebar.recent.groupBy.v2", "project");
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", title: "需要置顶的会话" }),
      session({ id: "s-2", title: "普通会话" }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([projectCard({ name: "关联项目" })]);
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "s-1" })]);

    renderSidebar({ view: "session", sessionId: "s-1" });
    await screen.findByText("需要置顶的会话");

    // Initially no pinned section
    expect(screen.queryByRole("button", { name: /置顶 \(/ })).toBeNull();

    // Click pin button on s-1
    const pinBtn = screen.getByRole("button", { name: "置顶会话：需要置顶的会话" });
    fireEvent.click(pinBtn);

    // Now pinned section appears with count 1
    const pinnedHeader = await screen.findByTitle("置顶 (1)");
    expect(pinnedHeader).toBeTruthy();

    // Verify localStorage persistence
    const stored = JSON.parse(localStorage.getItem("noending.sidebar.pinnedSessions") || "[]");
    expect(stored).toContain("s-1");

    // Current sessions and retained terminals must not duplicate the pinned row.
    expect(screen.getAllByText("需要置顶的会话")).toHaveLength(1);
    expect(screen.getAllByText("普通会话")).toHaveLength(1);
    expect(screen.getByTitle("关联项目 (1)")).toBeTruthy();

    // Unpin by clicking the pin button
    fireEvent.click(screen.getByRole("button", { name: "取消置顶：需要置顶的会话" }));

    // Pinned section disappears
    await waitFor(() => {
      expect(screen.queryByTitle(/置顶 \(/)).toBeNull();
    });

    const storedAfter = JSON.parse(localStorage.getItem("noending.sidebar.pinnedSessions") || "[]");
    expect(storedAfter).not.toContain("s-1");
    expect(screen.getAllByText("需要置顶的会话")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "置顶会话：需要置顶的会话" })).toBeTruthy();
    expect(screen.getByTitle("关联项目 (2)")).toBeTruthy();
  });

  it("keeps pinned sessions out of the regular list when the pinned section is collapsed", async () => {
    viewState.set("sidebar.recent.groupBy.v2", "project");
    localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["s-pinned"]));
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-pinned", title: "已置顶会话" }),
    ]);

    renderSidebar();
    const pinnedHeader = await screen.findByTitle("置顶 (1)");
    expect(pinnedHeader.getAttribute("aria-expanded")).toBe("true");

    // Click to collapse
    fireEvent.click(pinnedHeader);
    expect(pinnedHeader.getAttribute("aria-expanded")).toBe("false");

    // Collapsing the pinned section must not reveal a duplicate in the regular list.
    expect(screen.queryByText("已置顶会话")).toBeNull();
    expect(screen.queryByTitle("未知任务 (1)")).toBeNull();

    cleanup();
    viewState.clear();
    renderSidebar();
    const restoredPinnedHeader = await screen.findByTitle("置顶 (1)");
    expect(restoredPinnedHeader.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("已置顶会话")).toBeNull();

    fireEvent.click(restoredPinnedHeader);
    expect(restoredPinnedHeader.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getAllByText("已置顶会话")).toHaveLength(1);
  });

  it("excludes pinned terminal-only rows from the regular list and restores terminal navigation on unpin", async () => {
    localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["terminal:t-1"]));
    vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_title: "待同步会话", initial_workstream_ids: ["ws-1"] })]);
    vi.mocked(api.listProjectCards).mockResolvedValue([projectCard({ name: "启动任务" })]);
    const navigate = renderSidebar({ view: "terminal", terminalId: "t-1" });

    await screen.findByTitle("置顶 (1)");
    expect(screen.getAllByText("待同步会话")).toHaveLength(1);
    expect(screen.queryByTitle("启动任务 (1)")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "取消置顶：待同步会话" }));
    expect(screen.queryByTitle("置顶 (1)")).toBeNull();
    expect(await screen.findByTitle("无项目 (1)")).toBeTruthy();
    expect(screen.getAllByText("待同步会话")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /^待同步会话/ }));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t-1" }));
  });

  it("opens group dropdown menu, displays options and closes on selection or click outside", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", title: "测试会话" }),
    ]);

    renderSidebar();
    await screen.findByText("测试会话");

    const groupTrigger = screen.getByRole("button", { name: "会话归类：按项目归类" });
    expect(groupTrigger.getAttribute("aria-expanded")).toBe("false");

    // Click trigger to open menu
    fireEvent.click(groupTrigger);
    expect(groupTrigger.getAttribute("aria-expanded")).toBe("true");

    const projectOpt = screen.getByRole("menuitemradio", { name: "按项目归类" });
    const agentOpt = screen.getByRole("menuitemradio", { name: "按 Agent 归类" });
    const noneOpt = screen.getByRole("menuitemradio", { name: "不分组（全部会话）" });
    expect(projectOpt.getAttribute("aria-checked")).toBe("true");
    expect(agentOpt.getAttribute("aria-checked")).toBe("false");
    expect(noneOpt.getAttribute("aria-checked")).toBe("false");

    // Click outside closes the menu
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("menuitemradio", { name: "按项目归类" })).toBeNull();

    // Reopen and select "按项目归类"
    fireEvent.click(groupTrigger);
    fireEvent.click(screen.getByRole("menuitemradio", { name: "按项目归类" }));

    // Menu closes and trigger updates label
    expect(screen.queryByRole("menuitemradio", { name: "按项目归类" })).toBeNull();
    expect(screen.getByRole("button", { name: "会话归类：按项目归类" })).toBeTruthy();
    expect(screen.getByText("项目")).toBeTruthy();
  });

  it("renders flat session list without group headers when ungrouped (会话) is selected", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", title: "会话一", project_id: "ws-1" }),
      session({ id: "s-2", title: "会话二", project_id: "ws-2" }),
    ]);
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "任务一" }),
      projectCard({ id: "ws-2", name: "任务二" }),
    ]);

    renderSidebar();
    await screen.findByText("任务一");
    expect(screen.getByText("会话一")).toBeTruthy();
    expect(screen.getByText("会话二")).toBeTruthy();

    // Switch to ungrouped "不分组（全部会话）"
    switchGroupBy("不分组（全部会话）");

    // Group headers should no longer exist
    expect(screen.queryByText("任务一")).toBeNull();
    expect(screen.queryByText("任务二")).toBeNull();

    // Sessions should still be rendered in flat list
    expect(screen.getByText("会话一")).toBeTruthy();
    expect(screen.getByText("会话二")).toBeTruthy();

    // Group trigger now shows "会话"
    expect(screen.getByRole("button", { name: "会话归类：不分组（全部会话）" })).toBeTruthy();
    expect(screen.getByText("会话")).toBeTruthy();
  });

  it("switches sorting between 创建时间 and 最近更新 via combined menu", async () => {
    const olderCreatedRecentUpdated = session({
      id: "s-1",
      title: "老创建新聊天",
      started_at: "2026-10-01T10:00:00.000Z",
      last_conversation_at: "2026-10-10T12:00:00.000Z",
    });
    const newerCreatedOldUpdated = session({
      id: "s-2",
      title: "新创建老聊天",
      started_at: "2026-10-05T10:00:00.000Z",
      last_conversation_at: "2026-10-06T12:00:00.000Z",
    });

    vi.mocked(api.listSessions).mockResolvedValue([olderCreatedRecentUpdated, newerCreatedOldUpdated]);

    renderSidebar();
    await screen.findByText("老创建新聊天");

    // By default, sorted by createdAt desc: newerCreated first
    let sessionLinks = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-session-link"));
    expect(sessionLinks[0].textContent).toContain("新创建老聊天");
    expect(sessionLinks[1].textContent).toContain("老创建新聊天");

    // Open combined filter & sort menu
    const filterBtn = screen.getByRole("button", { name: "筛选与排序会话" });
    fireEvent.click(filterBtn);

    const sortCreatedRadio = screen.getByRole("menuitemradio", { name: "创建时间" });
    const sortUpdatedRadio = screen.getByRole("menuitemradio", { name: "最近更新" });
    expect(sortCreatedRadio.getAttribute("aria-checked")).toBe("true");
    expect(sortUpdatedRadio.getAttribute("aria-checked")).toBe("false");

    // Switch to 最近更新
    fireEvent.click(sortUpdatedRadio);
    expect(sortUpdatedRadio.getAttribute("aria-checked")).toBe("true");

    // Sessions now ordered by updatedAt desc: olderCreatedRecentUpdated first
    sessionLinks = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-session-link"));
    expect(sessionLinks[0].textContent).toContain("老创建新聊天");
    expect(sessionLinks[1].textContent).toContain("新创建老聊天");

    // Switch back to 创建时间
    fireEvent.click(sortCreatedRadio);
    sessionLinks = screen.getAllByRole("button").filter(b => b.className.includes("sidebar-session-link"));
    expect(sessionLinks[0].textContent).toContain("新创建老聊天");
    expect(sessionLinks[1].textContent).toContain("老创建新聊天");
  });

  it("limits group to 5 sessions with 展开显示 button and resets on collapse", async () => {
    const ws = projectCard({ id: "ws-test", name: "测试任务" });
    const testSessions = Array.from({ length: 7 }, (_, i) =>
      session({
        id: `s-${i + 1}`,
        title: `会话 ${i + 1}`,
        project_id: "ws-test",
        started_at: new Date(Date.now() - (7 - i) * 60000).toISOString(),
      }),
    );

    vi.mocked(api.listProjectCards).mockResolvedValue([ws]);
    vi.mocked(api.listSessions).mockResolvedValue(testSessions);

    renderSidebar();
    await screen.findByText("会话 7");

    // By default, only 5 sessions are visible
    expect(screen.queryByText("会话 7")).toBeTruthy();
    expect(screen.queryByText("会话 6")).toBeTruthy();
    expect(screen.queryByText("会话 5")).toBeTruthy();
    expect(screen.queryByText("会话 4")).toBeTruthy();
    expect(screen.queryByText("会话 3")).toBeTruthy();
    expect(screen.queryByText("会话 2")).toBeNull();
    expect(screen.queryByText("会话 1")).toBeNull();
    await waitFor(() => expect(new Set(vi.mocked(api.getSessionTerminalStates).mock.lastCall?.[0]))
      .toEqual(new Set(["s-3", "s-4", "s-5", "s-6", "s-7"])));

    // 展开显示 button is shown
    const expandBtn = screen.getByRole("button", { name: "展开显示" });
    expect(expandBtn).toBeTruthy();

    // Click 展开显示: reveals all 7 sessions and hides button
    fireEvent.click(expandBtn);
    expect(screen.getByText("会话 2")).toBeTruthy();
    expect(screen.getByText("会话 1")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "展开显示" })).toBeNull();
    await waitFor(() => expect(vi.mocked(api.getSessionTerminalStates).mock.lastCall?.[0]).toHaveLength(7));

    // Collapse the group
    const groupHeader = screen.getByRole("button", { name: /测试任务/ });
    fireEvent.click(groupHeader);
    expect(screen.queryByText("会话 7")).toBeNull();

    // Re-expand the group: resets to showing at most 5 sessions with 展开显示 button
    fireEvent.click(groupHeader);
    expect(screen.queryByText("会话 7")).toBeTruthy();
    expect(screen.queryByText("会话 3")).toBeTruthy();
    expect(screen.queryByText("会话 2")).toBeNull();
    expect(screen.queryByText("会话 1")).toBeNull();
    expect(screen.getByRole("button", { name: "展开显示" })).toBeTruthy();
    cleanup();

    // Group with <= 5 sessions does not show expand button
    vi.mocked(api.listSessions).mockResolvedValue(testSessions.slice(0, 4));
    renderSidebar();
    await screen.findByText("会话 4");
    expect(screen.queryByRole("button", { name: "展开显示" })).toBeNull();
  });

  it("collapses all groups and expands all groups with the one-click button", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "任务甲" }),
      projectCard({ id: "ws-2", name: "任务乙" }),
    ]);
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", project_id: "ws-1", title: "会话甲" }),
      session({ id: "s-2", project_id: "ws-2", title: "会话乙" }),
    ]);

    renderSidebar();

    expect(await screen.findByText("会话甲")).toBeTruthy();
    expect(screen.getByText("会话乙")).toBeTruthy();

    const collapseAllBtn = screen.getByRole("button", { name: "收起全部" });

    // Click collapse all
    fireEvent.click(collapseAllBtn);

    expect(screen.queryByText("会话甲")).toBeNull();
    expect(screen.queryByText("会话乙")).toBeNull();

    // Button should now be "展开全部"
    const expandAllBtn = screen.getByRole("button", { name: "展开全部" });

    // Click expand all
    fireEvent.click(expandAllBtn);

    expect(await screen.findByText("会话甲")).toBeTruthy();
    expect(screen.getByText("会话乙")).toBeTruthy();
    expect(screen.getByRole("button", { name: "收起全部" })).toBeTruthy();
  });

  it("toggles session icons display via display settings in menu", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      projectCard({ id: "ws-1", name: "任务分组" }),
    ]);
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", project_id: "ws-1", title: "有图标的会话", agent: "codex" }),
    ]);

    renderSidebar();

    await screen.findByText("有图标的会话");
    const sessionRow = screen.getByText("有图标的会话").closest(".sidebar-recent-item");
    expect(sessionRow?.querySelector(".agent-icon")).toBeTruthy();
    expect(document.querySelector(".sidebar-recent-group-items")?.classList.contains("has-icons")).toBe(true);

    // Open filter/sort menu
    const filterBtn = screen.getByRole("button", { name: /筛选与排序/ });
    fireEvent.click(filterBtn);

    expect(screen.getByText("显示设置")).toBeTruthy();
    const toggleIconBtn = screen.getByRole("menuitemcheckbox", { name: "显示会话图标" });
    expect(toggleIconBtn.getAttribute("aria-checked")).toBe("true");

    // Click to hide icons
    fireEvent.click(toggleIconBtn);
    expect(toggleIconBtn.getAttribute("aria-checked")).toBe("false");
    expect(sessionRow?.querySelector(".agent-icon")).toBeNull();
    expect(document.querySelector(".sidebar-recent-group-items")?.classList.contains("has-icons")).toBe(false);

    // Click to show icons again
    fireEvent.click(toggleIconBtn);
    expect(toggleIconBtn.getAttribute("aria-checked")).toBe("true");
    expect(sessionRow?.querySelector(".agent-icon")).toBeTruthy();
    expect(document.querySelector(".sidebar-recent-group-items")?.classList.contains("has-icons")).toBe(true);
  });

  it("disables one-click collapse button in ungrouped mode", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "s-1", title: "平铺会话" }),
    ]);

    renderSidebar();
    await screen.findByText("平铺会话");

    switchGroupBy("不分组（全部会话）");

    const collapseBtn = screen.getByRole("button", { name: "收起全部" });
    expect(collapseBtn.hasAttribute("disabled")).toBe(true);
  });
});

it("shows assistant and ordinary project sessions together with their terminals", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard({ id: "assistant-project", name: "我的助手", purpose: "assistant", search_paths: ["/home/app/assistant"] }),
    projectCard(),
  ]);
  vi.mocked(api.listSessions).mockResolvedValue([
    session({ id: "assistant-session", title: "助手原生会话", project_id: "assistant-project" }),
    session({ id: "regular-session", title: "普通会话" }),
  ]);
  vi.mocked(api.terminalList).mockResolvedValue([
    terminal({ terminal_id: "assistant-bound", session_id: "assistant-session", session_title: "助手原生会话" }),
    terminal({ terminal_id: "assistant-pending", cwd: "/home/app/assistant", session_title: "待绑定助手" }),
    terminal({ terminal_id: "regular-bound", session_id: "regular-session", session_title: "普通会话" }),
  ]);
  const navigate = renderSidebar({ view: "sessions" });
  await screen.findByText("助手原生会话");
  expect(screen.getByText("我的助手")).toBeTruthy();
  expect(screen.getByText("待绑定助手")).toBeTruthy();
  expect(screen.getByText("普通会话")).toBeTruthy();
  fireEvent.click(screen.getByText("助手原生会话"));
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "assistant-bound", initialSessionId: "assistant-session" }));
});


it("restores sidebar grouping, filters, running-only, sorting and display preferences after a restart", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([
    session({ id: "active", title: "未归档内容" }),
    session({ id: "idle", title: "未运行内容" }),
    session({ id: "archived", title: "归档内容", archived_at: new Date().toISOString() }),
  ]);
  vi.mocked(api.terminalList).mockResolvedValue([terminal({ session_id: "active" })]);
  renderSidebar();
  await screen.findByText("归档内容");
  switchGroupBy("按 Agent 归类");
  fireEvent.click(screen.getByRole("button", { name: "仅显示运行中会话" }));
  fireEvent.click(screen.getByRole("button", { name: "筛选与排序会话" }));
  fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "未归档" }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: "最近更新" }));
  fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "显示会话图标" }));
  cleanup();
  viewState.clear();

  renderSidebar();
  await screen.findByText("未归档内容");
  expect(screen.queryByText("归档内容")).toBeNull();
  expect(screen.queryByText("未运行内容")).toBeNull();
  expect(screen.getByRole("button", { name: "仅显示运行中会话" }).getAttribute("aria-pressed")).toBe("true");
  screen.getByRole("button", { name: "会话归类：按 Agent 归类" });
  expect(screen.queryByRole("menu")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "筛选与排序会话" }));
  expect(screen.getByRole("menuitemcheckbox", { name: "未归档" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("menuitemradio", { name: "最近更新" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("menuitemcheckbox", { name: "显示会话图标" }).getAttribute("aria-checked")).toBe("false");
});

it("immediately removes a deleted pinned session and all its terminals even while an old list refresh returns", async () => {
  localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["deleted", "kept"]));
  vi.mocked(api.listSessions).mockResolvedValue([session({ id: "deleted", title: "待删除" }), session({ id: "kept", title: "保留" })]);
  vi.mocked(api.terminalList).mockResolvedValue([terminal({ terminal_id: "closed", session_id: "deleted", live: false })]);
  renderSidebar();
  await screen.findByText("待删除");
  act(() => invalidateTargets({ sessionIds: ["deleted"], terminalIds: ["closed"] }));
  expect(screen.queryByText("待删除")).toBeNull();
  await act(async () => {});
  expect(screen.queryByText("待删除")).toBeNull();
  expect(screen.getByText("保留")).toBeTruthy();
  expect(JSON.parse(localStorage.getItem("noending.sidebar.pinnedSessions")!)).toEqual(["kept"]);
});

it("follows the selected terminal without aggregating peers or reloading all sessions", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([session({ id: "s1", title: "并行回答" })]);
  const a = terminal({ terminal_id: "a", session_id: "s1", activity: { state: "working", revision: 1, unread: false } });
  const b = terminal({ terminal_id: "b", session_id: "s1", activity: { state: "completed", revision: 2, unread: true } });
  vi.mocked(api.terminalList).mockResolvedValue([a, b]);
  sessionTerminals.set("s1", "b");
  const navigate = vi.fn();
  const view = render(<Sidebar route={{ view: "terminal", terminalId: "a" }} navigate={navigate} />);
  await screen.findByRole("img", { name: "正在处理" });
  expect(screen.getAllByRole("img")).toHaveLength(1);
  expect(screen.queryByRole("img", { name: /未查看/ })).toBeNull();
  const button = screen.getByRole("button", { name: "终端 · 已连接：并行回答" });
  expect(button.getAttribute("aria-busy")).toBe("false");
  const sessionQueries = vi.mocked(api.listSessions).mock.calls.length;
  vi.mocked(api.terminalList).mockResolvedValue([{ ...a, activity: { state: "waiting", revision: 3, unread: false } }, b]);
  act(() => window.dispatchEvent(new CustomEvent(EVT_TERMINAL_ACTIVITY)));
  await screen.findByRole("img", { name: "等待输入" });
  expect(screen.getByRole("button", { name: "终端 · 已连接：并行回答" })).toBe(button);
  rememberSessionPage("s1", { view: "terminal", terminalId: "b" });
  view.rerender(<Sidebar route={{ view: "terminal", terminalId: "b" }} navigate={navigate} />);
  await screen.findByRole("img", { name: "回答完成未查看" });
  view.rerender(<Sidebar route={{ view: "new-session" }} navigate={navigate} />);
  expect(screen.getByRole("img", { name: "回答完成未查看" })).toBeTruthy();
  // Keep the selected exited instance's status even while a peer is still live.
  vi.mocked(api.terminalList).mockResolvedValue([a, { ...b, live: false }]);
  act(() => window.dispatchEvent(new CustomEvent(EVT_TERMINAL_ACTIVITY)));
  await screen.findByRole("img", { name: "已退出 · 有未查看的回答" });
  expect(screen.getAllByRole("img")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "终端 · 已连接：并行回答" })).toBe(button);
  // Removing the selected instance falls back to a retained peer.
  vi.mocked(api.terminalList).mockResolvedValue([a]);
  act(() => window.dispatchEvent(new CustomEvent(EVT_TERMINAL_ACTIVITY)));
  await screen.findByRole("img", { name: "正在处理" });
  expect(vi.mocked(api.listSessions).mock.calls.length).toBe(sessionQueries);
});

it("keeps an old pinned conversation available beyond 500 summaries without checking every hidden row", async () => {
  const rows = Array.from({ length: 601 }, (_, i) => session({
    id: `many-${i}`, title: `历史会话 ${i}`, project_id: "many-project",
    started_at: new Date(2020, 0, 1, 0, i).toISOString(),
  }));
  localStorage.setItem("noending.sidebar.pinnedSessions", JSON.stringify(["many-0"]));
  vi.mocked(api.listSessions).mockResolvedValue(rows);
  vi.mocked(api.listProjectCards).mockResolvedValue([projectCard({ id: "many-project", name: "历史项目" })]);
  renderSidebar();
  await screen.findByText("历史会话 0");
  expect(screen.getByText("历史会话 600")).toBeTruthy();
  expect(screen.queryByText("历史会话 400")).toBeNull();
  await waitFor(() => expect(new Set(vi.mocked(api.getSessionTerminalStates).mock.lastCall?.[0]))
    .toEqual(new Set(["many-0", "many-596", "many-597", "many-598", "many-599", "many-600"])));
});
