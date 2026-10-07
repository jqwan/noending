import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewSessionView from "./NewSessionView";
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

const preparedFixture = {
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
    note: "使用默认工作路径",
  },
  runtime: { model: null, provider: null, effort: null },
  state_fingerprint: "fp-1",
  prepared_at: "2026-10-05T15:00:00Z",
} satisfies import("../../types").PreparedLaunch;

beforeEach(() => {
  vi.mocked(api.launchEmbeddedNew).mockReset();
  vi.mocked(api.cancelPrepared).mockClear();
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
  vi.mocked(api.prepareNewSession).mockReset().mockImplementation(async (agent, ownerId, cwd) => ({
    ...preparedFixture,
    agent,
    owner_workstream_id: ownerId,
    cwd: cwd ?? "/tmp/workspace",
  }));
});

afterEach(cleanup);

describe("NewSessionView explicit agent resolution", () => {
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
        note: "使用默认工作路径",
      },
      runtime: { model: null, provider: null, effort: null },
      state_fingerprint: "fp-claude",
      prepared_at: "2026-10-05T15:00:00Z",
    });

    render(<NewSessionView navigate={vi.fn()} agent="claude_code" />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", null);
    });

    expect(api.setDefaultAgent).not.toHaveBeenCalled();
    const select = screen.getByLabelText("Agent") as HTMLSelectElement;
    expect(select.value).toBe("claude_code");
  });

  it("falls back to default agent when no agent is specified", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null);
    });

    const select = screen.getByLabelText("Agent") as HTMLSelectElement;
    expect(select.value).toBe("codex");
  });

  it("keeps a manual Agent selection when the default Agent arrives late", async () => {
    let resolveDefault!: (agent: "codex") => void;
    vi.mocked(api.getDefaultAgent).mockReturnValue(new Promise((resolve) => { resolveDefault = resolve; }));
    render(<NewSessionView navigate={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "claude_code" } });
    resolveDefault("codex");
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", null));
    expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code");
  });

  it("only shows CLI agents in the dropdown options", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

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

  it("allows changing the selected agent on the page and prepares launch with the new agent", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

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

describe("NewSessionView working directory selection", () => {
  it("in standalone mode, defaults to NoEnding default workspace and lists existing working directories", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    const select = await screen.findByLabelText("工作路径") as HTMLSelectElement;

    await waitFor(() => {
      expect(select.value).toBe("/tmp/workspace");
    });

    const optionTexts = Array.from(select.querySelectorAll("option")).map((o) => o.textContent);
    expect(optionTexts.some((t) => t?.includes("/tmp/workspace (NoEnding 默认工作区)"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("/repo/existing-1 (Project 1)"))).toBe(true);
    expect(optionTexts.some((t) => t?.includes("浏览其他目录…"))).toBe(true);
  });

  it("in standalone mode, allows choosing another existing working directory and calls prepareNewSession with it", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    const select = await screen.findByLabelText("工作路径");
    fireEvent.change(select, { target: { value: "/repo/existing-1" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/repo/existing-1");
    });
  });

  it("when created from a task with paths, lists task working directories, defaults to primary path, and allows switching", async () => {
    vi.mocked(api.prepareNewSession).mockImplementation(async (_agent, _ownerId, cwd) => ({
      ...preparedFixture,
      cwd: cwd ?? "/repo/task-primary",
    }));
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

    render(<NewSessionView workstreamId="ws-1" navigate={vi.fn()} />);

    const select = await screen.findByLabelText("工作路径") as HTMLSelectElement;

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

    render(<NewSessionView workstreamId="ws-empty" navigate={vi.fn()} />);

    const select = await screen.findByLabelText("工作路径") as HTMLSelectElement;

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

  render(<NewSessionView navigate={navigate} />);
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalled());
  fireEvent.change(screen.getByRole("textbox", { name: "首条消息" }), { target: { value: "请检查这段代码\n并修复问题" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));

  await waitFor(() =>
    expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, undefined, "请检查这段代码\n并修复问题"),
  );
  expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-new-1" });
});
});


describe("NewSessionView message composer", () => {
  it("waits for a nonblank first message without creating a terminal on entry", async () => {
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.launchEmbeddedNew).not.toHaveBeenCalled();
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.change(screen.getByRole("textbox", { name: "首条消息" }), { target: { value: " \n " } });
    expect(send.disabled).toBe(true);
  });

  it("supports Enter to send, while Shift+Enter and IME Enter do not launch", async () => {
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
      launched_via: "内嵌终端", command_line: "codex", note: "已启动", launch_intent_id: null, terminal_id: "t-keyboard",
    });
    const navigate = vi.fn();
    render(<NewSessionView navigate={navigate} />);
    const input = screen.getByRole("textbox", { name: "首条消息" });
    fireEvent.change(input, { target: { value: "检查当前项目" } });
    await waitFor(() => expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
    expect(api.launchEmbeddedNew).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-keyboard" }));
    expect(api.launchEmbeddedNew).toHaveBeenCalledTimes(1);
  });

  it("keeps the message and selections after a failed launch and allows retry", async () => {
    vi.mocked(api.launchEmbeddedNew).mockRejectedValueOnce("CLI unavailable").mockResolvedValue({
      launched_via: "内嵌终端", command_line: "claude", note: "已启动", launch_intent_id: null, terminal_id: "t-retry",
    });
    const navigate = vi.fn();
    render(<NewSessionView navigate={navigate} />);
    await screen.findByRole("option", { name: "/repo/existing-1 (Project 1)" });
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "claude_code" } });
    fireEvent.change(screen.getByLabelText("工作路径"), { target: { value: "/repo/existing-1" } });
    const input = screen.getByRole("textbox", { name: "首条消息" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "  第一行\n第二行  " } });
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "启动失败：CLI unavailable重试");
    expect(input.value).toBe("  第一行\n第二行  ");
    expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code");
    expect((screen.getByLabelText("工作路径") as HTMLSelectElement).value).toBe("/repo/existing-1");
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-retry" }));
    expect(api.launchEmbeddedNew).toHaveBeenLastCalledWith("claude_code", null, "/repo/existing-1", "  第一行\n第二行  ");
    expect(api.cancelPrepared).toHaveBeenCalled();
  });
});


it("shows the resolved fallback directory and uses the same default-path intent when sending", async () => {
  vi.mocked(api.prepareNewSession).mockResolvedValue({
    ...preparedFixture,
    cwd: "/repo/fallback",
    cwd_resolution: { ...preparedFixture.cwd_resolution, cwd: "/repo/fallback", fallback: true, note: "主目录不可用，使用备用目录" },
  });
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
    launched_via: "内嵌终端", command_line: "codex", note: "已启动", launch_intent_id: null, terminal_id: "t-fallback",
  });
  render(<NewSessionView navigate={vi.fn()} />);
  const directory = screen.getByLabelText("工作路径") as HTMLSelectElement;
  await waitFor(() => expect(directory.value).toBe("/repo/fallback"));
  expect(screen.getByRole("status").textContent).toBe("主目录不可用，使用备用目录");
  fireEvent.change(directory, { target: { value: "/tmp/workspace" } });
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始任务" } });
  const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
  await waitFor(() => expect(send.disabled).toBe(false));
  fireEvent.change(directory, { target: { value: "/tmp/workspace" } });
  expect(send.disabled).toBe(false);
  fireEvent.click(send);
  await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, undefined, "开始任务"));
});


it("does not prepare another launch if the user leaves while startup fails", async () => {
  let rejectLaunch!: (reason: string) => void;
  vi.mocked(api.launchEmbeddedNew).mockReturnValue(new Promise((_resolve, reject) => { rejectLaunch = reject; }));
  const { unmount } = render(<NewSessionView navigate={vi.fn()} />);
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始任务" } });
  const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
  await waitFor(() => expect(send.disabled).toBe(false));
  fireEvent.click(send);
  const prepareCount = vi.mocked(api.prepareNewSession).mock.calls.length;
  unmount();
  await act(async () => { rejectLaunch("startup failed"); });
  expect(api.prepareNewSession).toHaveBeenCalledTimes(prepareCount);
});
