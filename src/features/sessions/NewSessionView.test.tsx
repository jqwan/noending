import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewSessionView from "./NewSessionView";
import { api } from "../../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { PathProbe } from "../../types";

const probeFixture: PathProbe = {raw: "/tmp/workspace", status: "ok", canonical_path: "/tmp/workspace", exists: true, git_state: "none", git_kind: null, project: null};

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
    listProjectCards: vi.fn(),
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
    addProjectPath: vi.fn(),
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
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

    note: "使用默认工作路径",
  },
  runtime: { model: null, provider: null, effort: null },
  state_fingerprint: "fp-1",
  prepared_at: "2026-10-05T15:00:00Z",
} satisfies import("../../types").PreparedLaunch;

beforeEach(() => {
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.addProjectPath).mockReset();
  vi.mocked(api.launchEmbeddedNew).mockReset();
  vi.mocked(api.probeWorkspacePath).mockReset().mockResolvedValue(probeFixture);
  vi.mocked(api.cancelPrepared).mockClear();
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
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

it("probes the effective default directory once while directory selection initializes", async () => {
  let finishProbe!: (value: PathProbe) => void;
  vi.mocked(api.probeWorkspacePath).mockImplementation(() => new Promise((resolve) => { finishProbe = resolve; }));
  render(<NewSessionView navigate={vi.fn()} />);
  await waitFor(() => {
    expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
    expect(api.probeWorkspacePath).toHaveBeenCalledTimes(1);
  });
  await act(async () => { finishProbe(probeFixture); });
  expect(api.probeWorkspacePath).toHaveBeenCalledTimes(1);
});

describe("NewSessionView task and project selection", () => {
  const mockProjects = [
    {
      id: "p-1",
      name: "Project 1",
      name_customized: false,
      kind: "git" as const,
      path_count: 1,
      missing_path_count: 0,
      workstream_count: 1,
      session_count: 5,
      representative_paths: ["/repo/project-1"],
      search_paths: ["/repo/project-1"],
      last_activity_at: null,
      updated_at: "2026-10-01T00:00:00Z",
    },
  ];


  it("renders + 新任务 in task options and selecting it opens the new task modal", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    const taskSelect = (await screen.findByLabelText("所属任务（可选）")) as HTMLSelectElement;
    fireEvent.change(taskSelect, { target: { value: "__new_workstream__" } });

    expect(await screen.findByRole("heading", { name: "新建任务" })).toBeTruthy();
  });

  it.each(["Codex Chat", "NoEnding Chat", "用户改名的聊天项目"])("excludes chat-directory project %s from selection", async (name) => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      ...mockProjects,
      {
        id: "p-codex",
        name,
        name_customized: false,
        kind: "chat_directory" as const,
        path_count: 1,
        missing_path_count: 0,
        workstream_count: 0,
        session_count: 0,
        representative_paths: ["/Documents/Codex"],
        search_paths: ["/Documents/Codex"],
        last_activity_at: null,
        updated_at: "2026-10-01T00:00:00Z",
      },
    ]);
    render(<NewSessionView navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;
    const options = Array.from(projectSelect.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toContain("Project 1");
    expect(options).not.toContain(name);
  });

  it("defaults to unassigned project and lists discovered projects and + 新项目", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    render(<NewSessionView navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;

    await waitFor(() => {
      expect(projectSelect.value).toBe("none");
    });

    const options = Array.from(projectSelect.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toContain("无项目");
    expect(options).toContain("Project 1");
    expect(options).toContain("+ 新项目");

    // Also renders directory picker
    const dirSelect = (await screen.findByLabelText("项目目录")) as HTMLSelectElement;
    expect(dirSelect.value).toBe("/tmp/workspace");
  });

  it("when choosing a project, sets working directory to project's first workspace path and calls prepareNewSession with it", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    render(<NewSessionView navigate={vi.fn()} />);

    const projectSelect = await screen.findByLabelText("所属项目");
    fireEvent.change(projectSelect, { target: { value: "p-1" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/repo/project-1");
    });
    expect(api.probeWorkspacePath).not.toHaveBeenCalledWith("/repo/project-1");
  });

  it("allows switching directory under a project with multiple paths", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      {
        id: "p-multi",
        name: "Multi Project",
        name_customized: false,
        kind: "git" as const,
        path_count: 2,
        missing_path_count: 0,
        workstream_count: 0,
        session_count: 0,
        representative_paths: ["/repo/main", "/repo/worktree-1"],
        search_paths: ["/repo/main", "/repo/worktree-1"],
        last_activity_at: null,
        updated_at: "2026-10-01T00:00:00Z",
      },
    ]);
    render(<NewSessionView navigate={vi.fn()} />);

    const projectSelect = await screen.findByLabelText("所属项目");
    fireEvent.change(projectSelect, { target: { value: "p-multi" } });

    const dirSelect = (await screen.findByLabelText("项目目录")) as HTMLSelectElement;
    await waitFor(() => {
      expect(dirSelect.value).toBe("/repo/main");
    });

    fireEvent.change(dirSelect, { target: { value: "/repo/worktree-1" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/repo/worktree-1");
    });
  });

  it("selecting + 新项目 triggers folder dialog and registers project", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    vi.mocked(open).mockResolvedValueOnce("/new/picked/repo");
    vi.mocked(api.addProjectPath).mockResolvedValueOnce({
      path: {
        id: "wp-new",
        canonical_path: "/new/picked/repo",
        project_id: "p-new",
        git_state: "detected",
        git_kind: "repo",
        exists: true,
        first_seen_at: "2026-10-01T00:00:00Z",
        last_seen_at: "2026-10-01T00:00:00Z",
      },
      project_id: "p-new",
      project_name: "picked",
    });

    render(<NewSessionView navigate={vi.fn()} />);
    const projectSelect = await screen.findByLabelText("所属项目");

    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: "__new_project__" } });
    });

    await waitFor(() => {
      expect(open).toHaveBeenCalled();
      expect(api.addProjectPath).toHaveBeenCalledWith("/new/picked/repo");
    });
  });

  it("can switch a task session from its project to the default directory without losing task ownership", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      {
        id: "ws-1",
        title: "我的功能开发",
        description: "",

        visibility: "normal",
        created_at: "2026-10-01T00:00:00Z",
        updated_at: "2026-10-01T00:00:00Z",
        projects: [{ id: "p-1", name: "Project 1" }],
        summary: null,
        last_activity_at: null,
        session_count: 1,
        latest_session: null,
        path_count: 1,

      },
    ]);

    render(<NewSessionView workstreamId="ws-1" navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;

    await waitFor(() => {
      expect(projectSelect.value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", "ws-1", "/repo/project-1");
    });

    fireEvent.change(projectSelect, { target: { value: "none" } });
    await waitFor(() => {
      expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", "ws-1");
    });
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
      launched_via: "内嵌终端", command_line: "codex", note: "已启动", terminal_id: "t-unassigned",
    });
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "继续这个任务" } });
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", "ws-1", undefined, "继续这个任务"));
  });

  it("allows a Git project named NoEnding Chat to be selected normally", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      { ...mockProjects[0], name: "NoEnding Chat", search_paths: ["/tmp/workspace"] },
    ]);
    render(<NewSessionView projectId="p-1" navigate={vi.fn()} />);
    await waitFor(() => {
      expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", null, "/tmp/workspace");
    });
  });

  it("when created with projectId, defaults to that project", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);

    render(<NewSessionView projectId="p-1" navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;

    await waitFor(() => {
      expect(projectSelect.value).toBe("p-1");
    });
  });

  it("launches embedded and navigates to the standalone terminal view", async () => {
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
    launched_via: "内嵌终端",
    command_line: "codex",
    note: "已在内嵌终端启动。",
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

it("renders inbox icon for unassigned task and project pickers and their menu items", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([{
    id: "ws-1", title: "测试任务", description: "", visibility: "normal",
    projects: [], created_at: "", updated_at: "", summary: null,
    last_activity_at: null, session_count: 0, latest_session: null, path_count: 0,
  }]);
  render(<NewSessionView navigate={vi.fn()} />);

  const taskPicker = await screen.findByRole("button", { name: /所属任务：无任务/ });
  const taskPickerIconPath = taskPicker.querySelector("svg path")?.getAttribute("d");
  expect(taskPickerIconPath).toBe("M3.5 3.5h11l1.5 6v4a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1v-4L3.5 3.5ZM2 9.5h4l1.5 2h3l1.5-2h4");

  const projectPicker = await screen.findByRole("button", { name: /所属项目：无项目/ });
  const projectPickerIconPath = projectPicker.querySelector("svg path")?.getAttribute("d");
  expect(projectPickerIconPath).toBe("M3.5 3.5h11l1.5 6v4a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1v-4L3.5 3.5ZM2 9.5h4l1.5 2h3l1.5-2h4");

  // Open task menu
  fireEvent.click(taskPicker);
  const unassignedTaskOption = screen.getByRole("button", { name: "无任务" });
  expect(unassignedTaskOption.querySelector(".menu-item-icon svg path")?.getAttribute("d")).toBe(
    "M3.5 3.5h11l1.5 6v4a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1v-4L3.5 3.5ZM2 9.5h4l1.5 2h3l1.5-2h4",
  );
  const realTaskOption = screen.getByRole("button", { name: "测试任务" });
  expect(realTaskOption.querySelector(".menu-item-icon svg path")?.getAttribute("d")).toBe(
    "M5 2.5h8M3.5 5h11M2.5 7.5h13v8h-13ZM5 10.5h5M5 13h8",
  );

  // Select real task
  fireEvent.click(realTaskOption);
  expect(taskPicker.querySelector("svg path")?.getAttribute("d")).toBe(
    "M5 2.5h8M3.5 5h11M2.5 7.5h13v8h-13ZM5 10.5h5M5 13h8",
  );

  // Open project menu
  fireEvent.click(projectPicker);
  const unassignedProjectOption = screen.getByRole("button", { name: "无项目" });
  expect(unassignedProjectOption.querySelector(".menu-item-icon svg path")?.getAttribute("d")).toBe(
    "M3.5 3.5h11l1.5 6v4a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1v-4L3.5 3.5ZM2 9.5h4l1.5 2h3l1.5-2h4",
  );
  const realProjectOption = screen.getByRole("button", { name: "Project 1" });
  expect(realProjectOption.querySelector(".menu-item-icon svg path")?.getAttribute("d")).toBe(
    "M9 2.5l6 3.5-6 3.5-6-3.5ZM3 6v6l6 3.5 6-3.5V6M9 9.5v6",
  );

  // Select real project
  fireEvent.click(realProjectOption);
  expect(projectPicker.querySelector("svg path")?.getAttribute("d")).toBe(
    "M9 2.5l6 3.5-6 3.5-6-3.5ZM3 6v6l6 3.5 6-3.5V6M9 9.5v6",
  );
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
      launched_via: "内嵌终端", command_line: "codex", note: "已启动", terminal_id: "t-keyboard",
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
    vi.mocked(api.listProjectCards).mockResolvedValue([
      {
        id: "p-1",
        name: "Project 1",
        name_customized: false,
        kind: "git" as const,
        path_count: 1,
        missing_path_count: 0,
        workstream_count: 1,
        session_count: 5,
        representative_paths: ["/repo/existing-1"],
        search_paths: ["/repo/existing-1"],
        last_activity_at: null,
        updated_at: "2026-10-01T00:00:00Z",
      },
    ]);
    vi.mocked(api.launchEmbeddedNew).mockRejectedValueOnce("CLI unavailable").mockResolvedValue({
      launched_via: "内嵌终端", command_line: "claude", note: "已启动", terminal_id: "t-retry",
    });
    const navigate = vi.fn();
    render(<NewSessionView navigate={navigate} />);
    await screen.findByRole("option", { name: "Project 1" });
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "claude_code" } });
    fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "p-1" } });
    const input = screen.getByRole("textbox", { name: "首条消息" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "  第一行\n第二行  " } });
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "启动失败：CLI unavailable重试");
    expect(input.value).toBe("  第一行\n第二行  ");
    expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code");
    expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1");
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
    cwd_resolution: { ...preparedFixture.cwd_resolution, cwd: "/repo/fallback", fallback: true, note: "工作目录不可用，使用备用目录" },
  });
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
    launched_via: "内嵌终端", command_line: "codex", note: "已启动", terminal_id: "t-fallback",
  });
  render(<NewSessionView navigate={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole("status").textContent).toBe("工作目录不可用，使用备用目录"));
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始任务" } });
  const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
  await waitFor(() => expect(send.disabled).toBe(false));
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

it("does not designate the first project of a multi-project task as its default", async () => {
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([{
    id: "multi", title: "跨项目任务", description: "", visibility: "normal",
    projects: [{ id: "p-1", name: "甲" }, { id: "p-2", name: "乙" }],
    created_at: "", updated_at: "", summary: null,
    last_activity_at: null, session_count: 0, latest_session: null, path_count: 2,
  }]);
  render(<NewSessionView workstreamId="multi" navigate={vi.fn()} />);
  const task = await screen.findByLabelText("所属任务（可选）");
  await waitFor(() => expect((task as HTMLSelectElement).value).toBe("multi"));
  expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("none");
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", "multi"));
});
