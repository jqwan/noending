import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AgentsView from "./AgentsView";
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
    desktop_app: "ChatGPT",
    desktop_app_present: true,
  },
  claude_code: {
    name: "Claude Code",
    detected: true,
    executable: "/opt/homebrew/bin/claude",
    version: "claude 1.0.0",
    terminal_cli: true,
    desktop_app: null,
    desktop_app_present: false,
  },
  antigravity: {
    name: "Antigravity",
    detected: true,
    executable: "/usr/local/bin/antigravity",
    version: "1.0",
    terminal_cli: true,
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
});

afterEach(cleanup);

describe("AgentsView", () => {
  it("renders centralized agent status and accurate discovered records metric without default agent controls", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    expect(await screen.findByText("代理")).toBeTruthy();
    // 验证口径：展示为同步发现 X 条记录，而非新会话数
    expect(await screen.findByText(/同步发现 5 条记录 · 写入 12 条新消息/)).toBeTruthy();

    // 不再展示默认 Agent 徽标及设为默认按钮
    expect(screen.queryByText("默认 Agent")).toBeNull();
    expect(screen.queryByRole("button", { name: "设为默认" })).toBeNull();

    // Antigravity 来源聚合与格式展示
    expect(screen.getByText("Desktop 格式")).toBeTruthy();
    expect(screen.getByText("CLI 格式")).toBeTruthy();
  });

  it("renders capability badges (TUI) without new session buttons on cards", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    await screen.findByText("Claude Code");
    // 代理卡片上不再展示新建会话按钮
    expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
    // 绝无设为默认按钮
    expect(screen.queryByRole("button", { name: "设为默认" })).toBeNull();
  });

  it("renders sources with no toggle checkbox and displays total sources metric", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    // No checkbox in source rows
    expect(screen.queryByRole("checkbox")).toBeNull();
    // Metric indicates total configured sources
    expect(await screen.findByText(/共 3 个来源目录/)).toBeTruthy();
  });

  it("opens modal on re-ingest explaining safe rescan", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    const reingestButtons = await screen.findAllByRole("button", { name: "全量同步" });
    fireEvent.click(reingestButtons[0]);

    expect(await screen.findByText(/绝不会清库重建或删除已有记录/)).toBeTruthy();
    const confirmBtn = screen.getAllByRole("button", { name: "全量同步" });
    // 弹窗内的确认按钮
    fireEvent.click(confirmBtn[confirmBtn.length - 1]);

    await waitFor(() => {
      expect(api.reingestSource).toHaveBeenCalled();
    });
  });

  it("opens native folder picker when clicking + 添加来源目录 and adds source", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    vi.mocked(open).mockResolvedValue("/custom/sessions/path");
    vi.mocked(api.addIngestSource).mockResolvedValue({
      id: "src-new",
      agent: "claude_code",
      path: "/custom/sessions/path",
      enabled: true,
      origin: "user",
      exists: true,
      created_at: "2026-10-05T15:00:00Z",
    });

    render(<AgentsView navigate={vi.fn()} />);

    const addButtons = await screen.findAllByRole("button", { name: "+ 添加来源目录" });
    fireEvent.click(addButtons[0]);

    await waitFor(() => {
      expect(open).toHaveBeenCalled();
      expect(api.addIngestSource).toHaveBeenCalledWith("codex", "/custom/sessions/path");
    });
  });

  it("renders no open-method toggles: every format has exactly one continue method", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    // 打开方式不再是偏好：内嵌退役后每种格式只有一种「继续」方式
    // （桌面应用，或内嵌终端——后者的入口在会话页终端子页），
    // 卡片只展示事实徽标，不再提供切换组。
    await screen.findByText("Codex");
    expect(screen.queryByRole("group", { name: "打开方式切换" })).toBeNull();
  });

  it("renders fixed resume badges for Antigravity desktop and CLI formats without toggle switches", async () => {
    const { container } = render(<AgentsView navigate={vi.fn()} />);

    // Antigravity displays both formats
    expect(await screen.findByText("Desktop 格式")).toBeTruthy();
    expect(await screen.findByText("CLI 格式")).toBeTruthy();

    const antigravityCard = container.querySelector("#agent-card-antigravity");
    expect(antigravityCard).toBeTruthy();

    // Antigravity formats have static badges, not toggle buttons
    const staticBadges = antigravityCard!.querySelectorAll('.badge[title="打开方式"]');
    expect(staticBadges.length).toBe(2); // One for Desktop, one for CLI
    expect(Array.from(staticBadges, (badge) => badge.textContent)).toEqual(["Desktop", "TUI"]);

    // Antigravity card has NO toggle switch
    expect(antigravityCard!.querySelector('[aria-label="打开方式切换"]')).toBeNull();
  });

  it("renders filter tabs (全部, TUI, 桌面端) without 已就绪 or 桌面与历史, and switches filters properly", async () => {
    render(<AgentsView navigate={vi.fn()} />);

    // Tabs exist
    expect(await screen.findByRole("tab", { name: /全部/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /TUI/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /桌面端/ })).toBeTruthy();

    // Removed old tabs
    expect(screen.queryByRole("tab", { name: /已就绪/ })).toBeNull();
    expect(screen.queryByRole("tab", { name: /桌面与历史/ })).toBeNull();

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

  it("renders icon buttons for copy, incremental sync, and remove in source rows", async () => {
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

    render(<AgentsView navigate={vi.fn()} />);

    // Copy icon button
    const copyBtn = await screen.findByRole("button", { name: "复制完整路径" });
    expect(copyBtn).toBeTruthy();

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
});
