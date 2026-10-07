import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewSessionModal from "./NewSessionModal";
import { api } from "../../api";

const mockAgentStatus = {
  codex: {
    name: "Codex",
    detected: true,
    executable: "/usr/local/bin/codex",
    version: "0.1.0",
    terminal_cli: true,
    desktop_app: null,
    desktop_app_present: false,
    resume_open_method: "terminal" as const,
  },
  claude_code: {
    name: "Claude Code",
    detected: true,
    executable: "/usr/local/bin/claude",
    version: "1.0.0",
    terminal_cli: true,
    desktop_app: null,
    desktop_app_present: false,
    resume_open_method: "terminal" as const,
  },
  pi: {
    name: "Pi",
    detected: true,
    executable: "/usr/local/bin/pi",
    version: "0.2.0",
    terminal_cli: true,
    desktop_app: null,
    desktop_app_present: false,
    resume_open_method: "terminal" as const,
  },
  antigravity: {
    name: "Antigravity",
    detected: true,
    executable: "/usr/local/bin/agy",
    version: "2.0.0",
    terminal_cli: true,
    desktop_app: "Antigravity",
    desktop_app_present: true,
    resume_open_method: "terminal" as const,
  },
  workbuddy: {
    name: "WorkBuddy",
    detected: false,
    executable: null,
    version: null,
    terminal_cli: false,
    desktop_app: "WorkBuddy",
    desktop_app_present: true,
    resume_open_method: "desktop" as const,
  },
  qoder: {
    name: "Qoder",
    detected: false,
    executable: null,
    version: null,
    terminal_cli: false,
    desktop_app: null,
    desktop_app_present: false,
    resume_open_method: "terminal" as const,
  },
  dsh: {
    name: "DSH",
    detected: false,
    executable: null,
    version: null,
    terminal_cli: false,
    desktop_app: null,
    desktop_app_present: false,
    resume_open_method: "terminal" as const,
  },
  zcode: {
    name: "ZCode",
    detected: false,
    executable: null,
    version: null,
    terminal_cli: false,
    desktop_app: "ZCode",
    desktop_app_present: true,
    resume_open_method: "desktop" as const,
  },
};

vi.mock("../../api", () => ({
  api: {
    listWorkstreamCards: vi.fn(),
    getAgentStatus: vi.fn(),
    getDefaultAgent: vi.fn(),
    setDefaultAgent: vi.fn(),
    prepareNewSession: vi.fn(),
    cancelPrepared: vi.fn().mockResolvedValue(undefined),
    launchPrepared: vi.fn(),
    launchEmbeddedNew: vi.fn(),
    getWorkspaceSettings: vi.fn(),
    listRecentWorkspacePaths: vi.fn(),
    listWorkstreamPaths: vi.fn(),
  },
}));

beforeEach(() => {
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.getAgentStatus).mockReset().mockResolvedValue(mockAgentStatus);
  vi.mocked(api.getDefaultAgent).mockReset().mockResolvedValue("codex");
  vi.mocked(api.setDefaultAgent).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.getWorkspaceSettings).mockReset().mockResolvedValue({
    noending_home: "/tmp",
    default_workspace: "/tmp/workspace",
    pending_home: null,
    restart_required: false,
    db_path: "/tmp/db.sqlite",
    home_source: "bootstrap",
  });
  vi.mocked(api.listRecentWorkspacePaths).mockReset().mockResolvedValue([
    {
      path: "/repo/existing-1",
      known: true,
      exists: true,
      project_name: "Project 1",
      git_state: null,
      git_kind: null,
      last_used_at: null,
    },
  ]);
  vi.mocked(api.listWorkstreamPaths).mockReset().mockResolvedValue([]);
  vi.mocked(api.prepareNewSession).mockReset().mockResolvedValue({
    id: "prep-1",
    mode: "new",
    agent: "codex",
    owner_workstream_id: null,
    cwd: "/tmp/workspace",
    cwd_resolution: {
      source: "default_workspace",
      cwd: "/tmp/workspace",
      fallback: false,
      workstream_id: null,
      path_position: null,
      note: "使用默认工作目录",
    },
    runtime: { model: null, provider: null, effort: null },
    state_fingerprint: "fp-1",
    prepared_at: "2026-10-05T15:00:00Z",
  });
});

afterEach(cleanup);

describe("NewSessionModal explicit agent resolution", () => {
  it("uses explicitly specified agent over default agent and does not mutate global default setting", async () => {
    vi.mocked(api.prepareNewSession).mockResolvedValueOnce({
      id: "prep-claude",
      mode: "new",
      agent: "claude_code",
      owner_workstream_id: null,
      cwd: "/tmp/workspace",
      cwd_resolution: {
        source: "default_workspace",
        cwd: "/tmp/workspace",
        fallback: false,
        workstream_id: null,
        path_position: null,
        note: "使用默认工作目录",
      },
      runtime: { model: null, provider: null, effort: null },
      state_fingerprint: "fp-claude",
      prepared_at: "2026-10-05T15:00:00Z",
    });

    render(<NewSessionModal onClose={vi.fn()} agent="claude_code" />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", null);
    });

    expect(api.setDefaultAgent).not.toHaveBeenCalled();
    const select = screen.getByLabelText("Agent") as HTMLSelectElement;
    expect(select.value).toBe("claude_code");
  });

  it("falls back to default agent when no agent is specified", async () => {
    render(<NewSessionModal onClose={vi.fn()} />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null);
    });

    const select = screen.getByLabelText("Agent") as HTMLSelectElement;
    expect(select.value).toBe("codex");
  });

  it("only shows CLI agents in the dropdown options", async () => {
    render(<NewSessionModal onClose={vi.fn()} />);

    const select = await screen.findByLabelText("Agent");
    const optionTexts = Array.from(select.querySelectorAll("option")).map((o) => o.textContent);

    // 必须包含 CLI Agents
    expect(optionTexts.some((t) => t?.includes("Codex"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("Claude Code"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("Pi"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("Antigravity"))).toBe(true);

    // 绝不包含纯桌面或无 CLI Agent
    expect(optionTexts.some((t) => t?.includes("WorkBuddy"))).toBe(false);
    expect(optionTexts.some((t) => t?.includes("Qoder"))).toBe(false);
    expect(optionTexts.some((t) => t?.includes("DSH"))).toBe(false);
    expect(optionTexts.some((t) => t?.includes("ZCode"))).toBe(false);
  });

  it("allows changing the selected agent in the modal and prepares launch with the new agent", async () => {
    render(<NewSessionModal onClose={vi.fn()} />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null);
    });

    const select = screen.getByLabelText("Agent");
    fireEvent.change(select, { target: { value: "claude_code" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", null);
    });

    // 单次切换绝不改动全局默认配置
    expect(api.setDefaultAgent).not.toHaveBeenCalled();
  });
});

describe("NewSessionModal working directory selection", () => {
  it("in standalone mode, defaults to NoEnding default workspace and lists existing working directories", async () => {
    render(<NewSessionModal onClose={vi.fn()} />);

    const select = await screen.findByLabelText("工作目录") as HTMLSelectElement;

    await waitFor(() => {
      expect(select.value).toBe("/tmp/workspace");
    });

    const optionTexts = Array.from(select.querySelectorAll("option")).map((o) => o.textContent);
    expect(optionTexts.some((t) => t?.includes("/tmp/workspace (NoEnding 默认工作区)"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("/repo/existing-1 (Project 1)"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("浏览其他目录…"))).toBe(true);
  });

  it("in standalone mode, allows choosing another existing working directory and calls prepareNewSession with it", async () => {
    render(<NewSessionModal onClose={vi.fn()} />);

    const select = await screen.findByLabelText("工作目录");
    fireEvent.change(select, { target: { value: "/repo/existing-1" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/repo/existing-1");
    });
  });

  it("when created from a task with paths, lists task working directories, defaults to primary path, and allows switching", async () => {
    vi.mocked(api.listWorkstreamPaths).mockResolvedValue([
      {
        id: "wp-1",
        workstream_id: "ws-1",
        workspace_path_id: "p-1",
        canonical_path: "/repo/task-primary",
        position: 0,
        project_id: "prj-1",
        project_name: "Task Project",
        exists: true,
        created_at: "2026-09-21T00:00:00Z",
      },
      {
        id: "wp-2",
        workstream_id: "ws-1",
        workspace_path_id: "p-2",
        canonical_path: "/repo/task-secondary",
        position: 1,
        project_id: "prj-1",
        project_name: "Task Project",
        exists: true,
        created_at: "2026-09-21T00:00:00Z",
      },
    ]);

    render(<NewSessionModal workstreamId="ws-1" onClose={vi.fn()} />);

    const select = await screen.findByLabelText("工作目录") as HTMLSelectElement;

    await waitFor(() => {
      expect(select.value).toBe("/repo/task-primary");
    });

    const optionTexts = Array.from(select.querySelectorAll("option")).map((o) => o.textContent);
    expect(optionTexts.some((t) => t?.includes("/repo/task-primary (主目录)"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("/repo/task-secondary"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("浏览其他目录…"))).toBe(false);

    fireEvent.change(select, { target: { value: "/repo/task-secondary" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", "ws-1", "/repo/task-secondary");
    });
  });

  it("when created from a task with NO paths, defaults to NoEnding default workspace", async () => {
    vi.mocked(api.listWorkstreamPaths).mockResolvedValue([]);

    render(<NewSessionModal workstreamId="ws-empty" onClose={vi.fn()} />);

    const select = await screen.findByLabelText("工作目录") as HTMLSelectElement;

    await waitFor(() => {
      expect(select.value).toBe("/tmp/workspace");
    });

    const optionTexts = Array.from(select.querySelectorAll("option")).map((o) => o.textContent);
    expect(optionTexts.some((t) => t?.includes("/tmp/workspace (NoEnding 默认工作区)"))).toBe(true);
  });
it("launches embedded and navigates to the standalone terminal view", async () => {
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
    launched_via: "内嵌终端",
    command_line: "codex",
    note: "已在内嵌终端启动。",
    launch_intent_id: "intent-1",
    terminal_id: "t-new-1",
  });
  const navigate = vi.fn();
  const onClose = vi.fn();

  render(<NewSessionModal onClose={onClose} navigate={navigate} />);
  await screen.findByText("新建会话");
  fireEvent.click(screen.getByRole("button", { name: "启动" }));

  await waitFor(() =>
    expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, undefined),
  );
  expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-new-1" });
  expect(onClose).toHaveBeenCalled();
});
});
