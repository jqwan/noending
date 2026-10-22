import { viewState } from "../../hooks/useViewState";
import { clearRuntimeCatalog } from "./useAgentRuntime";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runtimeFixture } from "./runtimeFixtures";
import AgentsView, { clearAgentsCache } from "./AgentsView";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import type { AgentStatusEntry, IngestSource, IngestTaskStatus } from "../../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("../../api", () => ({
  api: { matchObjects: vi.fn().mockResolvedValue([]),
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    getAgentStatus: vi.fn(),
    getAgentRuntimeSettings: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
    setAgentLaunchEnabled: vi.fn(),
    redetectAgent: vi.fn(),
    getDefaultAgent: vi.fn(),
    setDefaultAgent: vi.fn(),
    listIngestSources: vi.fn(),
    addIngestSource: vi.fn(),
    removeIngestSource: vi.fn(),
    reconcileSource: vi.fn(),
    reingestSource: vi.fn(),
    reconcileAll: vi.fn(),
    getIngestionStatus: vi.fn(),
    setResumeOpenMethod: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listSessions: vi.fn(),
    getSessionCountsByAgent: vi.fn(),
    getAgentLatestActivities: vi.fn(),
    prepareNewSession: vi.fn(),
    cancelPrepared: vi.fn(),
  },
}));

const mockAgentStatus: Record<string, AgentStatusEntry> = {
  codex: {
    name: "Codex",
    detected: true,
    executable: "/usr/local/bin/codex",
    version: "codex 0.1.0",
    terminal_cli: true,
    launch_enabled: true,
    desktop_app: "ChatGPT",
    desktop_app_present: true,
  },
  claude_code: {
    name: "Claude Code",
    detected: true,
    executable: "/opt/homebrew/bin/claude",
    version: "claude 1.0.0",
    terminal_cli: true,
    launch_enabled: true,
    desktop_app: null,
    desktop_app_present: false,
  },
  antigravity: {
    name: "Antigravity",
    detected: true,
    executable: "/usr/local/bin/antigravity",
    version: "1.0",
    terminal_cli: true,
    launch_enabled: true,
    desktop_app: "Antigravity",
    desktop_app_present: true,
  },
  pi: {
    name: "Pi",
    detected: false,
    executable: null,
    version: null,
    terminal_cli: true,
    launch_enabled: false,
    desktop_app: null,
    desktop_app_present: false,
  },
};

const mockSources: IngestSource[] = [
  {
    id: "src-1",
    agent: "claude_code",
    path: "~/.claude/projects",
    origin: "default",
    exists: true,
  },
  {
    id: "src-2",
    agent: "antigravity",
    path: "~/.gemini/antigravity",
    origin: "default",
    exists: true,
  },
  {
    id: "src-3",
    agent: "antigravity",
    path: "~/.gemini/antigravity-cli",
    origin: "default",
    exists: true,
  },
];

const mockIngestStatus: IngestTaskStatus = {
  scope: "reconcile_all",
  started_at: "2026-10-05T14:00:00Z",
  finished_at: "2026-10-05T14:01:00Z",
  discovered: 5,
  messages: 12,
  error: null,
};

beforeEach(() => {
  viewState.clear();
  localStorage.clear();
  clearRuntimeCatalog();
  clearAgentsCache();
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async (agent) => runtimeFixture(agent));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockResolvedValue({ ...runtimeFixture(), model_source: "dynamic", models: [{ id: "listed-model", display_name: "Listed", provider: null, supported_efforts: ["low"] }] });
  vi.mocked(api.setAgentLaunchEnabled).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.redetectAgent).mockReset().mockResolvedValue(runtimeFixture());
  vi.mocked(api.addIngestSource).mockReset();
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.getAgentStatus).mockReset().mockResolvedValue(mockAgentStatus);
  vi.mocked(api.getDefaultAgent).mockReset().mockResolvedValue("codex");
  vi.mocked(api.setDefaultAgent).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.listIngestSources).mockReset().mockResolvedValue(mockSources);
  vi.mocked(api.getIngestionStatus).mockReset().mockResolvedValue(mockIngestStatus);
  vi.mocked(api.reconcileSource).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.reingestSource).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.reconcileAll).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.removeIngestSource).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listSessions).mockReset().mockResolvedValue([]);
  vi.mocked(api.getSessionCountsByAgent).mockReset().mockResolvedValue({});
  vi.mocked(api.getAgentLatestActivities).mockReset().mockResolvedValue({});
});

afterEach(cleanup);

describe("AgentsView", () => {
  it("renders synchronization counts, configured source counts and both Antigravity formats", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    expect(await screen.findByText("Agent")).toBeTruthy();
    // 验证口径：展示为同步发现 X 条记录，而非新会话数
    expect(await screen.findByText(/同步发现 5 条记录 · 写入 12 条新消息/)).toBeTruthy();

    expect(await screen.findByRole("combobox", { name: "状态" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "全部" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "已就绪" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "未就绪" })).toBeTruthy();

    expect(screen.getByRole("button", { name: "查看 Antigravity Agent 详情" })).toBeTruthy();
    expect(screen.queryByText("~/.gemini/antigravity-cli")).toBeNull();
  });


  it("opens modal on re-ingest explaining safe rescan", async () => {
    render(<AgentsView navigate={vi.fn()} initialAgent="claude_code" />);

    const reingestButtons = await screen.findAllByRole("button", { name: "全量同步" });
    fireEvent.click(reingestButtons[0]);

    expect(await screen.findByText(/将清理已失效的旧消息/)).toBeTruthy();
    expect(api.reingestSource).not.toHaveBeenCalled();
    const confirmBtn = screen.getAllByRole("button", { name: "全量同步" });
    // 弹窗内的确认按钮
    fireEvent.click(confirmBtn[confirmBtn.length - 1]);

    await waitFor(() => {
      expect(api.reingestSource).toHaveBeenCalledWith("src-1");
    });
  });

  it("opens native folder picker when clicking + 添加来源目录 and adds source", async () => {
    vi.mocked(open).mockResolvedValue("/custom/sessions/path");
    vi.mocked(api.addIngestSource).mockResolvedValue({
      id: "src-new",
      agent: "codex",
      path: "/custom/sessions/path",
      origin: "user",
      exists: true,
    });

    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);

    const addButtons = await screen.findAllByRole("button", { name: "+ 添加来源目录" });
    fireEvent.click(addButtons[0]);

    await waitFor(() => {
      expect(open).toHaveBeenCalled();
      expect(api.addIngestSource).toHaveBeenCalledWith("codex", "/custom/sessions/path");
    });
  });

  it("filters Agents by terminal and desktop availability and readiness", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    // Top-right surface buttons exist with TUI active by default
    const tuiBtn = await screen.findByRole("button", { name: "TUI" });
    const desktopBtn = screen.getByRole("button", { name: "桌面端" });
    expect(tuiBtn.getAttribute("aria-pressed")).toBe("true");
    expect(desktopBtn.getAttribute("aria-pressed")).toBe("false");

    // Readiness select exists with options
    const statusSelect = screen.getByRole("combobox", { name: "状态" });
    expect(statusSelect).toBeTruthy();

    // Default TUI shows TUI agents
    expect(screen.getByText("Claude Code")).toBeTruthy();
    expect(screen.getByText("Codex")).toBeTruthy();
    expect(screen.getByText("Pi")).toBeTruthy();

    // Filter by unready in TUI: Pi has detected: false
    fireEvent.change(statusSelect, { target: { value: "unready" } });
    expect(screen.getByText("Pi")).toBeTruthy();
    expect(screen.queryByText("Claude Code")).toBeNull();

    // Filter by ready in TUI: Claude Code and Codex are ready
    fireEvent.change(statusSelect, { target: { value: "ready" } });
    expect(screen.getByText("Claude Code")).toBeTruthy();
    expect(screen.getByText("Codex")).toBeTruthy();
    expect(screen.queryByText("Pi")).toBeNull();

    // Reset to all
    fireEvent.change(statusSelect, { target: { value: "all" } });
    expect(screen.getByText("Claude Code")).toBeTruthy();

    // Switch to desktop surface
    fireEvent.click(desktopBtn);
    expect(desktopBtn.getAttribute("aria-pressed")).toBe("true");
    expect(tuiBtn.getAttribute("aria-pressed")).toBe("false");

    // Claude Code has desktop_app: null -> hidden in desktop surface
    expect(screen.queryByText("Claude Code")).toBeNull();
    // Codex has desktop_app: "ChatGPT" -> visible
    expect(screen.getByText("Codex")).toBeTruthy();
  });

  it("dispatches incremental sync and removal for the selected custom source", async () => {
    vi.mocked(api.listIngestSources).mockResolvedValue([
      {
        id: "src-user-1",
        agent: "claude_code",
        path: "/custom/path",
        origin: "user",
        exists: true,
      },
    ]);

    render(<AgentsView navigate={vi.fn()} initialAgent="claude_code" />);

    await screen.findByRole("button", { name: "增量同步" });

    // Incremental sync icon button
    const syncBtn = screen.getByRole("button", { name: "增量同步" });
    expect(syncBtn).toBeTruthy();
    fireEvent.click(syncBtn);
    await waitFor(() => {
      expect(api.reconcileSource).toHaveBeenCalledWith("src-user-1");
    });

    // Remove icon button (only for origin: "user")
    const removeBtn = screen.getByRole("button", { name: "移除此自定义目录" });
    expect(removeBtn).toBeTruthy();
    fireEvent.click(removeBtn);
    await waitFor(() => {
      expect(api.removeIngestSource).toHaveBeenCalledWith("src-user-1");
    });
  });

  it("opens agent details from the overview, triggers card sync, and separates Antigravity source formats", async () => {
    const navigate = vi.fn();
    const view = render(<AgentsView navigate={navigate} />);
    await screen.findByText("Claude Code");

    // Card incremental sync
    const syncBtn = screen.getByRole("button", { name: "增量同步 Claude Code" });
    fireEvent.click(syncBtn);
    await waitFor(() => expect(api.reconcileSource).toHaveBeenCalledWith("src-1"));

    // Navigate to details
    fireEvent.click(await screen.findByRole("button", { name: "查看 Antigravity Agent 详情" }));
    expect(navigate).toHaveBeenCalledWith({ view: "agents", agent: "antigravity" });
    view.unmount();

    render(<AgentsView navigate={navigate} initialAgent="antigravity" />);
    expect(await screen.findByText("Desktop 格式")).toBeTruthy();
    expect(screen.getByText("CLI 格式")).toBeTruthy();
    expect(screen.getByText("~/.gemini/antigravity-cli")).toBeTruthy();
  });

  it("restores the catalog after restart, replaces successful results and keeps them on failure", async () => {
    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);
    expect(await screen.findByText("模型与思考等级")).toBeTruthy();
    expect(api.refreshAgentRuntimeOptions).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "刷新模型列表" }));
    await screen.findByText(/来自 CLI · 1 个模型/);
    expect(await screen.findByText("Listed")).toBeTruthy();
    expect(screen.getByText(/思考档位：low/)).toBeTruthy();
    cleanup();
    viewState.clear(); // Restart memory, retain persistent preferences.
    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);
    expect(await screen.findByText("Listed")).toBeTruthy();
    expect(api.refreshAgentRuntimeOptions).toHaveBeenCalledTimes(1);

    vi.mocked(api.refreshAgentRuntimeOptions).mockResolvedValue({ ...runtimeFixture(), model_source: "dynamic", models: [{ id: "replacement", display_name: "Replacement", provider: null, supported_efforts: ["high"] }] });
    fireEvent.click(screen.getByRole("button", { name: "刷新模型列表" }));
    await screen.findByText("Replacement");
    expect(screen.queryByText("Listed")).toBeNull();
    expect(screen.getByText(/思考档位：high/)).toBeTruthy();

    vi.mocked(api.refreshAgentRuntimeOptions).mockResolvedValue({ ...runtimeFixture(), model_source: "unavailable", models: [], warnings: ["CLI offline"] });
    fireEvent.click(screen.getByRole("button", { name: "刷新模型列表" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Error: CLI offline");
    expect(screen.getByText("Replacement")).toBeTruthy();
    cleanup();
    viewState.clear();
    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);
    expect(await screen.findByText("Replacement")).toBeTruthy();
  });

  it("displays agent detail sessions with new session action and repairs unreadable defaults", async () => {
    const navigate = vi.fn();
    render(<AgentsView navigate={navigate} initialAgent="codex" />);
    expect(await screen.findByText("模型与思考等级")).toBeTruthy();
    expect(screen.getByText("会话（0）")).toBeTruthy();
    expect(screen.getByText("该 Agent 下还没有会话。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", agent: "codex" });

    // Displays alert when runtime settings fail to load
    cleanup();
    vi.mocked(api.getAgentRuntimeSettings).mockRejectedValueOnce(new Error("配置损坏"));
    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);
    expect((await screen.findByRole("alert")).textContent).toContain("配置损坏");
  });

  it("displays normalized CLI status, session count and toggles between card and list view", async () => {
    vi.mocked(api.getSessionCountsByAgent).mockResolvedValue({ codex: 602 });
    render(<AgentsView navigate={vi.fn()} />);
    await screen.findByText("Claude Code");
    expect(screen.getByText("Claude Code CLI v1.0.0")).toBeTruthy();
    expect(screen.getByText("Codex CLI v0.1.0")).toBeTruthy();
    expect(screen.getByText("602 个会话")).toBeTruthy();
    expect(api.listSessions).not.toHaveBeenCalled();
    expect(screen.queryByText(/仅同步历史/)).toBeNull();

    // Toggle to list view and back
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("Codex CLI v0.1.0")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("offers history controls without launch settings for ingestion-only agents", async () => {
    render(<AgentsView navigate={vi.fn()} initialAgent="dsh" />);
    await screen.findByText("该 Agent 不支持新建终端会话，可同步历史会话。");
    expect(screen.queryByText("模型与思考等级")).toBeNull();
    expect(api.getAgentRuntimeSettings).not.toHaveBeenCalled();
  });

  it("displays 未检测到CLI when an agent with terminal CLI is not detected", async () => {
    const statusCopy = {
      ...mockAgentStatus,
      codex: { ...mockAgentStatus.codex, detected: false, version: null },
    };
    vi.mocked(api.getAgentStatus).mockResolvedValue(statusCopy);
    render(<AgentsView navigate={vi.fn()} />);
    const codexCard = await screen.findByRole("button", { name: "查看 Codex Agent 详情" });
    expect(within(codexCard).getByText("未检测到CLI")).toBeTruthy();
    expect(screen.queryByTitle("来源目录")).toBeNull();
  });
});

it("uses lookup aliases while keeping the selected capability filter", async () => {
  vi.mocked(api.matchObjects).mockResolvedValue(["claude_code"]);
  render(<AgentsView navigate={vi.fn()} />);
  await screen.findByRole("button", { name: "查看 Claude Code Agent 详情" });
  fireEvent.change(screen.getByRole("textbox", {name:"搜索 Agent"}), {target:{value:"claude"}});
  await screen.findByRole("button", { name: "查看 Claude Code Agent 详情" });
  expect(screen.queryByRole("button", {name:"查看 Codex Agent 详情"})).toBeNull();
  expect(api.matchObjects).toHaveBeenCalledWith("agent","claude", expect.any(Array));
  fireEvent.click(screen.getByRole("button", { name: "桌面端" }));
  expect(screen.queryByRole("button", { name: "查看 Claude Code Agent 详情" })).toBeNull();
});

it("sorts agents by recent activity, name, and session count", async () => {
  vi.mocked(api.getAgentLatestActivities).mockResolvedValue({
    pi: "2026-10-17T01:00:00Z",
    claude_code: "2026-10-16T01:00:00Z",
  });
  vi.mocked(api.getSessionCountsByAgent).mockResolvedValue({
    antigravity: 10,
    codex: 5,
  });
  const { container } = render(<AgentsView navigate={vi.fn()} />);
  await screen.findByRole("button", { name: "查看 Pi Agent 详情" });

  // Default is "recent": Pi is latest active -> first card
  const cards = container.querySelectorAll(".agent-card");
  expect(cards[0].getAttribute("aria-label")).toContain("Pi");
  expect(cards[1].getAttribute("aria-label")).toContain("Claude Code");

  // Switch to "sessions"
  fireEvent.change(screen.getByRole("combobox", { name: "排序" }), {
    target: { value: "sessions" },
  });
  const cardsBySessions = container.querySelectorAll(".agent-card");
  expect(cardsBySessions[0].getAttribute("aria-label")).toContain("Antigravity");
  expect(cardsBySessions[1].getAttribute("aria-label")).toContain("Codex");

  // Switch to "name"
  fireEvent.change(screen.getByRole("combobox", { name: "排序" }), {
    target: { value: "name" },
  });
  const cardsByName = container.querySelectorAll(".agent-card");
  expect(cardsByName[0].getAttribute("aria-label")).toContain("Antigravity");
  expect(cardsByName[1].getAttribute("aria-label")).toContain("Claude Code");
});

it("syncs all registered sources and keeps the button busy while queued", async () => {
  vi.mocked(api.listIngestSources).mockResolvedValue(mockSources);
  vi.mocked(api.getAgentStatus).mockResolvedValue({});
  render(<AgentsView navigate={vi.fn()} />);
  const button = await screen.findByRole("button", { name: "增量同步全部" });
  vi.mocked(api.getIngestionStatus).mockResolvedValue({ scope: "reconcile_all", started_at: null, finished_at: null, discovered: 0, messages: 0, error: null });
  fireEvent.click(button);
  await waitFor(() => expect(api.reconcileAll).toHaveBeenCalledWith());
  await waitFor(() => expect(screen.getByText(/最近一次同步：增量扫描全部来源 · 正在同步中/)).toBeTruthy());
  expect(screen.queryByText("后台处理中…")).toBeNull();
  expect((button as HTMLButtonElement).disabled).toBe(true);
  expect(api.reconcileSource).not.toHaveBeenCalled();
});

it("keeps the Agent detail's sync scoped to its own sources", async () => {
  vi.mocked(api.listIngestSources).mockResolvedValue(mockSources);
  render(<AgentsView navigate={vi.fn()} initialAgent="antigravity" />);
  const button = await screen.findByRole("button", { name: "增量同步全部" });
  await screen.findByText("~/.gemini/antigravity-cli");
  fireEvent.click(button);
  await waitFor(() => expect(api.reconcileSource).toHaveBeenCalledTimes(2));
  expect(api.reconcileSource).toHaveBeenCalledWith("src-2");
  expect(api.reconcileSource).toHaveBeenCalledWith("src-3");
  expect(api.reconcileAll).not.toHaveBeenCalled();
});

it("renders immediately without flash when cached data is available", async () => {
  const view1 = render(<AgentsView navigate={vi.fn()} />);
  await screen.findByText("Claude Code CLI v1.0.0");
  view1.unmount();

  // Second render has data in cache immediately, rendering without delay or placeholder
  const view2 = render(<AgentsView navigate={vi.fn()} />);
  expect(screen.getByText("Claude Code CLI v1.0.0")).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "状态" })).toBeTruthy();
  view2.unmount();
});
