import { viewState } from "../../hooks/useViewState";
import { clearRuntimeCatalog } from "./useAgentRuntime";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runtimeFixture } from "./runtimeFixtures";
import AgentsView from "./AgentsView";
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
  api: {
    getAgentStatus: vi.fn(),
    getAgentRuntimeSettings: vi.fn(),
    setAgentRuntimeOverrides: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
    setAgentLaunchEnabled: vi.fn(),
    redetectAgent: vi.fn(),
    terminalList: vi.fn(),
    getDefaultAgent: vi.fn(),
    setDefaultAgent: vi.fn(),
    listIngestSources: vi.fn(),
    addIngestSource: vi.fn(),
    removeIngestSource: vi.fn(),
    setIngestSourceEnabled: vi.fn(),
    reconcileSource: vi.fn(),
    reingestSource: vi.fn(),
    reconcileAll: vi.fn(),
    getIngestionStatus: vi.fn(),
    setResumeOpenMethod: vi.fn(),
    listWorkstreamCards: vi.fn(),
    listSessions: vi.fn(),
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
};

const mockSources: IngestSource[] = [
  {
    id: "src-1",
    agent: "claude_code",
    path: "~/.claude/projects",
    enabled: true,
    origin: "default",
    exists: true,
    created_at: "2026-10-05T14:00:00Z",
  },
  {
    id: "src-2",
    agent: "antigravity",
    path: "~/.gemini/antigravity",
    enabled: true,
    origin: "default",
    exists: true,
    created_at: "2026-10-05T14:00:00Z",
  },
  {
    id: "src-3",
    agent: "antigravity",
    path: "~/.gemini/antigravity-cli",
    enabled: true,
    origin: "default",
    exists: true,
    created_at: "2026-10-05T14:00:00Z",
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
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async (agent) => runtimeFixture(agent));
  vi.mocked(api.setAgentRuntimeOverrides).mockReset().mockImplementation(async (agent, overrides) => ({ ...runtimeFixture(agent), overrides }));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockResolvedValue({ ...runtimeFixture(), model_source: "dynamic", models: [{ id: "listed-model", display_name: "Listed", provider: null, supported_efforts: ["low"] }] });
  vi.mocked(api.setAgentLaunchEnabled).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.redetectAgent).mockReset().mockResolvedValue(runtimeFixture());
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
  vi.mocked(api.addIngestSource).mockReset();
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.getAgentStatus).mockReset().mockResolvedValue(mockAgentStatus);
  vi.mocked(api.getDefaultAgent).mockReset().mockResolvedValue("codex");
  vi.mocked(api.setDefaultAgent).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.listIngestSources).mockReset().mockResolvedValue(mockSources);
  vi.mocked(api.getIngestionStatus).mockReset().mockResolvedValue(mockIngestStatus);
  vi.mocked(api.setIngestSourceEnabled).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.reconcileSource).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.reingestSource).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.reconcileAll).mockReset().mockResolvedValue({ queued: true });
  vi.mocked(api.removeIngestSource).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listSessions).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

describe("AgentsView", () => {
  it("renders synchronization counts, configured source counts and both Antigravity formats", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    expect(await screen.findByText("代理")).toBeTruthy();
    // 验证口径：展示为同步发现 X 条记录，而非新会话数
    expect(await screen.findByText(/同步发现 5 条记录 · 写入 12 条新消息/)).toBeTruthy();

    expect(await screen.findByText(/共 3 个来源目录/)).toBeTruthy();

    expect(screen.getByRole("button", { name: "查看 Antigravity 代理详情" })).toBeTruthy();
    expect(screen.queryByText("~/.gemini/antigravity-cli")).toBeNull();
  });


  it("opens modal on re-ingest explaining safe rescan", async () => {
    render(<AgentsView navigate={vi.fn()} initialAgent="claude_code" />);

    const reingestButtons = await screen.findAllByRole("button", { name: "全量同步" });
    fireEvent.click(reingestButtons[0]);

    expect(await screen.findByText(/绝不会清库重建或删除已有记录/)).toBeTruthy();
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
      enabled: true,
      origin: "user",
      exists: true,
      created_at: "2026-10-05T15:00:00Z",
    });

    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);

    const addButtons = await screen.findAllByRole("button", { name: "+ 添加来源目录" });
    fireEvent.click(addButtons[0]);

    await waitFor(() => {
      expect(open).toHaveBeenCalled();
      expect(api.addIngestSource).toHaveBeenCalledWith("codex", "/custom/sessions/path");
    });
  });

  it("filters Agents by terminal and desktop availability", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    // Tabs exist
    expect(await screen.findByRole("tab", { name: /全部/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /TUI/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /桌面端/ })).toBeTruthy();


    // Click TUI filter
    fireEvent.click(screen.getByRole("tab", { name: /TUI/ }));
    // Claude Code has terminal_cli: true -> visible
    expect(screen.getByText("Claude Code")).toBeTruthy();

    // Click 桌面端 filter
    fireEvent.click(screen.getByRole("tab", { name: /桌面端/ }));
    // Claude Code has desktop_app: null -> hidden in desktop filter
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
        enabled: true,
        origin: "user",
        exists: true,
        created_at: "2026-10-05T14:00:00Z",
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

  it("toggles between card and list view", async () => {
    const { container } = render(<AgentsView navigate={vi.fn()} />);
    await screen.findByText("Claude Code");

    expect(container.querySelector(".agents-grid")).toBeTruthy();
    expect(container.querySelector(".agents-list")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(container.querySelector(".agents-list")).toBeTruthy();
    expect(container.querySelector(".agents-grid")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(container.querySelector(".agents-grid")).toBeTruthy();
    expect(container.querySelector(".agents-list")).toBeNull();
  });
  it("opens agent details from the overview and separates Antigravity source formats", async () => {
    const navigate = vi.fn();
    const view = render(<AgentsView navigate={navigate} />);
    fireEvent.click(await screen.findByRole("button", { name: "查看 Antigravity 代理详情" }));
    expect(navigate).toHaveBeenCalledWith({ view: "agents", agent: "antigravity" });
    view.unmount();
    render(<AgentsView navigate={navigate} initialAgent="antigravity" />);
    expect(await screen.findByText("Desktop 格式")).toBeTruthy();
    expect(screen.getByText("CLI 格式")).toBeTruthy();
    expect(screen.queryByText("~/.claude/projects")).toBeNull();
    fireEvent.click(screen.getByLabelText("自动同步 ~/.gemini/antigravity-cli"));
    await waitFor(() => expect(api.setIngestSourceEnabled).toHaveBeenCalledWith("src-3", false));
    expect(api.setAgentLaunchEnabled).not.toHaveBeenCalled();
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

  it("triggers incremental scan for a single agent from the card action", async () => {
    render(<AgentsView navigate={vi.fn()} />);
    await screen.findByText("Claude Code");
    const syncBtn = screen.getByRole("button", { name: "增量同步 Claude Code" });
    fireEvent.click(syncBtn);
    await waitFor(() => expect(api.reconcileSource).toHaveBeenCalledWith("src-1"));
  });

  it("displays agent detail header with icon, models, and sessions section with new session action", async () => {
    const navigate = vi.fn();
    render(<AgentsView navigate={navigate} initialAgent="codex" />);
    expect(await screen.findByText("模型与思考等级")).toBeTruthy();
    expect(screen.getByText("会话 (0)")).toBeTruthy();
    expect(screen.getByText("该代理下还没有会话。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", agent: "codex" });
  });

  it("displays normalized CLI status and does not include 仅同步历史 on cards", async () => {
    render(<AgentsView navigate={vi.fn()} />);
    await screen.findByText("Claude Code");
    expect(screen.getByText("Claude Code CLI v1.0.0")).toBeTruthy();
    expect(screen.getByText("Codex CLI v0.1.0")).toBeTruthy();
    expect(screen.queryByText(/仅同步历史/)).toBeNull();
  });

  it("offers history controls without launch settings for ingestion-only agents", async () => {
    render(<AgentsView navigate={vi.fn()} initialAgent="dsh" />);
    await screen.findByText("该代理不支持新建终端会话，可同步历史会话。");
    expect(screen.queryByText("模型与思考等级")).toBeNull();
    expect(screen.queryByLabelText("用于新建会话")).toBeNull();
    expect(api.getAgentRuntimeSettings).not.toHaveBeenCalled();
  });

  it("can repair unreadable defaults by restoring native parameters", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockRejectedValue(new Error("配置损坏"));
    render(<AgentsView navigate={vi.fn()} initialAgent="codex" />);
    fireEvent.click(await screen.findByRole("button", { name: "恢复 Agent 默认参数" }));
    await waitFor(() => expect(api.setAgentRuntimeOverrides).toHaveBeenCalledWith("codex", { model: null, provider: null, effort: null }));
    expect(await screen.findByText("模型与思考等级")).toBeTruthy();
  });

  it("displays session count on agent cards", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      { id: "s1", agent: "codex" } as any,
      { id: "s2", agent: "codex" } as any,
    ]);
    render(<AgentsView navigate={vi.fn()} />);
    await screen.findByText("Codex");
    expect(await screen.findByText("2 个会话")).toBeTruthy();
  });
});
