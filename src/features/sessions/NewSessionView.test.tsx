import { viewState } from "../../hooks/useViewState";
import { NATIVE_RUNTIME } from "../agents/RuntimeFields";
import { clearRuntimeCatalog } from "../agents/useAgentRuntime";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runtimeFixture } from "../agents/runtimeFixtures";
import NewSessionView from "./NewSessionView";
import { api } from "../../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { PathProbe, ProjectCardData, WorkstreamCardData } from "../../types";

const probeFixture: PathProbe = {raw: "/tmp/workspace", status: "ok", canonical_path: "/tmp/workspace", exists: true, git_state: "none", git_kind: null, project: null};

/** 任务选项收在 picker 里（项目下的任务数量没有上限），断言 checkbox 前先展开它。 */
const openTaskMenu = () => fireEvent.click(screen.getByRole("button", { name: /^跟踪任务/ }));

/** 「新建任务」动作挂在菜单底部，和项目菜单的「新增项目」一致。 */
const openNewTaskForm = () => {
  openTaskMenu();
  fireEvent.click(screen.getByRole("button", { name: "新建任务" }));
};

const mockAgentStatus = {
  codex: {
    name: "Codex",
    detected: true,
    executable: "/usr/local/bin/codex",
    version: "0.1.0",
    terminal_cli: true,
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
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
    launch_enabled: true,
    desktop_app: "ZCode",
    desktop_app_present: true,
    resume_open_method: "desktop" as const,
  },
};

vi.mock("../../api", () => ({
  api: {
    listWorkstreamCards: vi.fn(),
    createWorkstream: vi.fn(),
    listProjectCards: vi.fn(),
    getAgentStatus: vi.fn(),
    getAgentRuntimeSettings: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
    getDefaultAgent: vi.fn(),
    setDefaultAgent: vi.fn(),
    prepareNewSession: vi.fn(),
    cancelPrepared: vi.fn().mockResolvedValue(undefined),
    launchPrepared: vi.fn(),
    launchEmbeddedNew: vi.fn(),
    getWorkspaceSettings: vi.fn(),
    listRecentWorkspacePaths: vi.fn(),

    addProjectPath: vi.fn(),
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

const chatProject: ProjectCardData = {
  id: "chat", name: "NoEnding Chat", name_customized: false, visibility: "normal",
  kind: "directory_group", purpose: "chat", path_count: 1, missing_path_count: 0,
  workstream_count: 0, session_count: 0, representative_paths: ["/tmp/workspace"],
  search_paths: ["/tmp/workspace"], last_activity_at: null, created_at: "", updated_at: "",
};

const preparedFixture = {
  id: "prep-1",
  mode: "new",
  agent: "codex",
  initial_workstream_ids: [],
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
  viewState.clear();
  localStorage.clear();
  clearRuntimeCatalog();
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async (agent) => runtimeFixture(agent));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockRejectedValue(new Error("catalog unavailable"));
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.addProjectPath).mockReset();
  vi.mocked(api.launchEmbeddedNew).mockReset();
  vi.mocked(api.probeWorkspacePath).mockReset().mockResolvedValue(probeFixture);
  vi.mocked(api.cancelPrepared).mockClear();
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([chatProject]);
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

  vi.mocked(api.prepareNewSession).mockReset().mockImplementation(async (agent, ownerId, cwd) => ({
    ...preparedFixture,
    agent,
    initial_workstream_ids: ownerId,
    cwd: cwd ?? "/tmp/workspace",
  }));
});

afterEach(cleanup);

describe("New session optional first message", () => {
  it.each(["", "  \n\t"])("opens the terminal without a prompt for blank input %j", async (message) => {
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "empty-terminal", launched_via: "embedded", command_line: "codex", note: "" });
    const navigate = vi.fn();
    render(<NewSessionView navigate={navigate} />);
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: message } });
    const openTerminal = screen.getByRole("button", { name: "打开终端" });
    await waitFor(() => expect(openTerminal).toHaveProperty("disabled", false));
    fireEvent.click(openTerminal);
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", undefined, NATIVE_RUNTIME));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "empty-terminal" });
  });

  it("passes a long first message intact to the backend file transport", async () => {
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "long-terminal", launched_via: "embedded", command_line: "codex", note: "" });
    render(<NewSessionView navigate={vi.fn()} />);
    const input = screen.getByLabelText("首条消息");
    const message = "中".repeat(1998) + "😀!";
    fireEvent.change(input, { target: { value: message } });
    await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).toHaveProperty("disabled", false));
    expect(input).toHaveProperty("value", message);
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", message, NATIVE_RUNTIME));
  });
});

describe("NewSessionView explicit agent resolution", () => {
  it("resolves explicitly specified agent over default agent and falls back without mutating global setting", async () => {
    vi.mocked(api.prepareNewSession).mockResolvedValueOnce({
      id: "prep-claude",
      mode: "new",
      agent: "claude_code",
      initial_workstream_ids: [],
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

    const { unmount } = render(<NewSessionView navigate={vi.fn()} agent="claude_code" />);
    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", [], "/tmp/workspace", NATIVE_RUNTIME);
    });
    expect(api.setDefaultAgent).not.toHaveBeenCalled();
    expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code");
    unmount();

    // Fallback when nothing is remembered and no agent is specified.
    localStorage.clear();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME);
    });
    expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("codex");
  });

  it("keeps a manual Agent selection when the default Agent arrives late", async () => {
    let resolveDefault!: (agent: "codex") => void;
    vi.mocked(api.getDefaultAgent).mockReturnValue(new Promise((resolve) => { resolveDefault = resolve; }));
    render(<NewSessionView navigate={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "claude_code" } });
    resolveDefault("codex");
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", [], "/tmp/workspace", NATIVE_RUNTIME));
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
      name_customized: false, visibility: "normal" as const,
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
    chatProject,
  ];


  it("opens the new task form from the inline tracking list", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    await waitFor(() => expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
    openNewTaskForm();

    expect(await screen.findByRole("heading", { name: "新建任务" })).toBeTruthy();
  });

  it("does not select a newly created task when the preset project association was removed", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    const created = {id:"removed",title:"独立任务",description:"",visibility:"normal" as const,created_at:"",updated_at:""};
    vi.mocked(api.createWorkstream).mockResolvedValue(created);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([]);
    render(<NewSessionView projectId="p-1" navigate={vi.fn()}/>);
    await screen.findByRole("option", { name: "Project 1" });
    openTaskMenu();
    const addTask = screen.getByRole("button", { name: "新建任务" });
    await waitFor(() => expect(addTask).not.toHaveProperty("disabled", true));
    fireEvent.click(addTask);
    fireEvent.click(await screen.findByRole("button", {name:"移除项目 Project 1"}));
    fireEvent.change(screen.getByRole("textbox", {name:"标题"}), {target:{value:"独立任务"}});
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([{...created,projects:[],last_activity_at:null,session_count:0,latest_session:null}]);
    fireEvent.submit(screen.getByRole("textbox", {name:"标题"}).closest("form")!);
    await waitFor(()=>expect(api.createWorkstream).toHaveBeenCalledWith("独立任务","",[]));
    await waitFor(()=>expect(screen.queryByRole("dialog")).toBeNull());
    openTaskMenu();
    expect(screen.queryByRole("checkbox", {name:"独立任务"})).toBeNull();
  });

  it("includes chat-directory projects in selection options", async () => {
    const chatNames = ["Codex Chat", "NoEnding Chat", "用户改名的聊天项目"];
    const extraProjects = chatNames.map((name, idx) => ({
      id: `p-chat-${idx}`,
      name,
      name_customized: false, visibility: "normal" as const,
      kind: "directory_group" as const,
      path_count: 1,
      missing_path_count: 0,
      workstream_count: 0,
      session_count: 0,
      representative_paths: [`/Documents/Chat-${idx}`],
      search_paths: [`/Documents/Chat-${idx}`],
      last_activity_at: null,
      updated_at: "2026-10-01T00:00:00Z",
    }));

    vi.mocked(api.listProjectCards).mockResolvedValue([
      ...mockProjects,
      ...extraProjects,
    ]);
    render(<NewSessionView navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;
    const options = Array.from(projectSelect.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toContain("Project 1");
    for (const name of chatNames) {
      expect(options).toContain(name);
    }
  });

  it("defaults by workspace identity even for a renamed project", async () => {
    const renamed = { ...chatProject, name: "我的聊天空间", name_customized: true, kind: "directory_group" as const,
      search_paths: ["/tmp/workspace/another", "/tmp/workspace"] };
    vi.mocked(api.listProjectCards).mockResolvedValue([mockProjects[0], renamed]);
    render(<NewSessionView navigate={vi.fn()} />);
    const project = screen.getByLabelText("所属项目") as HTMLSelectElement;
    await waitFor(() => expect(project.value).toBe("chat"));
    expect(screen.queryByRole("option", { name: "无项目" })).toBeNull();
    expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME));
    expect(api.addProjectPath).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "所属项目：我的聊天空间" }));
    expect(screen.queryByRole("button", { name: "无项目" })).toBeNull();
  });

  it("registers the default directory when it has no project yet", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([mockProjects[0]]).mockResolvedValue(mockProjects);
    vi.mocked(api.addProjectPath).mockResolvedValue({project_id: "chat", path: {canonical_path: "/tmp/workspace"}} as never);
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.addProjectPath).toHaveBeenCalledWith("/tmp/workspace"));
    await waitFor(() => expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME));
  });

  it("when choosing a project, sets working directory to first path and allows switching", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      chatProject,
      {
        id: "p-multi",
        name: "Multi Project",
        name_customized: false, visibility: "normal" as const,
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
    await screen.findByRole("option", { name: "Multi Project" });
    fireEvent.change(projectSelect, { target: { value: "p-multi" } });

    const dirSelect = (await screen.findByLabelText("项目目录")) as HTMLSelectElement;
    await waitFor(() => {
      expect(dirSelect.value).toBe("/repo/main");
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", [], "/repo/main", NATIVE_RUNTIME);
    });
    expect(api.probeWorkspacePath).not.toHaveBeenCalledWith("/repo/main");

    fireEvent.change(dirSelect, { target: { value: "/repo/worktree-1" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", [], "/repo/worktree-1", NATIVE_RUNTIME);
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

  it("can switch a task session from its project to the default directory and clears its incompatible tracked task", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      {
        id: "ws-1",
        title: "我的功能开发",
        description: "",

        visibility: "normal" as const,
        created_at: "2026-10-01T00:00:00Z",
        updated_at: "2026-10-01T00:00:00Z",
        projects: [{ id: "p-1", name: "Project 1" }],

        last_activity_at: null,
        session_count: 1,
        latest_session: null,


      },
    ]);

    render(<NewSessionView workstreamId="ws-1" navigate={vi.fn()} />);

    await waitFor(() => {
      expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", ["ws-1"], "/repo/project-1", NATIVE_RUNTIME);
    });

    // The task preset also selects its project, and both are remembered.
    cleanup();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => {
      expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", ["ws-1"], "/repo/project-1", NATIVE_RUNTIME);
    });

    fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "chat" } });
    await waitFor(() => {
      expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME);
    });
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
      launched_via: "内嵌终端", command_line: "codex", note: "已启动", terminal_id: "t-unassigned",
    });
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "继续这个任务" } });
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", "继续这个任务", NATIVE_RUNTIME));
  });

  it("when created with projectId, defaults to that project even if named NoEnding Chat", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      { ...mockProjects[0], name: "NoEnding Chat", search_paths: ["/tmp/workspace"] },
    ]);
    render(<NewSessionView projectId="p-1" navigate={vi.fn()} />);
    await waitFor(() => {
      expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME);
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
    expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", "请检查这段代码\n并修复问题", NATIVE_RUNTIME),
  );
  expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-new-1" });
});

it("offers only tasks associated with the selected project", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([
    { id:"matching",title:"可跟踪任务",visibility:"normal",projects:[{id:"p-1",name:"Project 1"}] },
    { id:"outside",title:"其他项目任务",visibility:"normal",projects:[{id:"p-2",name:"Project 2"}] },
    { id:"archived",title:"归档任务",visibility:"archived",projects:[{id:"p-1",name:"Project 1"}] },
  ] as WorkstreamCardData[]);
  render(<NewSessionView navigate={vi.fn()}/>);

  await waitFor(()=>expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
  openTaskMenu();
  expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  expect(screen.getByText("该项目下还没有跟踪任务")).toBeTruthy();
  await act(async()=>{fireEvent.change(screen.getByLabelText("所属项目"), {target:{value:"p-1"}});});
  openTaskMenu();
  expect(screen.getAllByRole("checkbox")).toHaveLength(1);
  expect(screen.getByRole("checkbox", {name:"可跟踪任务"})).toBeTruthy();
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
  await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", "开始任务", NATIVE_RUNTIME));
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

it("uses the first project of a multi-project task as its default", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([chatProject,{id:"p-1",name:"甲",search_paths:["/repo/project-1"],representative_paths:["/repo/project-1"]}] as never);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([{
    id: "multi", title: "跨项目任务", description: "", visibility: "normal" as const,
    projects: [{ id: "p-1", name: "甲" }, { id: "p-2", name: "乙" }],
    created_at: "", updated_at: "",
    last_activity_at: null, session_count: 0, latest_session: null,
  }]);
  render(<NewSessionView workstreamId="multi" navigate={vi.fn()} />);

  await waitFor(() => expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("p-1"));
  openTaskMenu();
  expect(screen.getByRole("checkbox", {name:"跨项目任务"})).toHaveProperty("checked", true);
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", ["multi"], "/repo/project-1", NATIVE_RUNTIME));
});

it("blocks starting from a task that has no associated projects", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([chatProject]);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([{
    id: "orphan", title: "无项目任务", description: "", visibility: "normal" as const,
    projects: [],
    created_at: "", updated_at: "",
    last_activity_at: null, session_count: 0, latest_session: null,
  }]);
  render(<NewSessionView workstreamId="orphan" navigate={vi.fn()} />);

  expect(await screen.findByText("该任务未关联项目，无法新建会话；请先在任务中关联项目。")).toBeTruthy();
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始" } });
  expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.launchEmbeddedNew).not.toHaveBeenCalled();
});

describe("NewSessionView runtime selection", () => {
  it("remembers runtime per agent as soon as it changes without changing global overrides", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockImplementation(async (agent) => ({
      ...runtimeFixture(agent),
      models: [{ id: "one-time-model", display_name: "One Time Model", provider: null, supported_efforts: ["low", "high"] }],
    }));
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "new-runtime", launched_via: "embedded", command_line: "test", note: "" });
    render(<NewSessionView navigate={vi.fn()} />);
    expect(screen.getByRole("heading", { name: "新建会话" })).toBeTruthy();

    const runtimeBtn = screen.getByRole("button", { name: "模型与推理设置" });
    await waitFor(() => expect(api.getAgentRuntimeSettings).toHaveBeenCalled());
    expect(runtimeBtn.textContent).toContain("Agent 默认");
    expect(runtimeBtn.textContent).not.toContain("One Time Model");

    fireEvent.click(runtimeBtn);
    expect(await screen.findByRole("dialog", { name: "模型与推理设置" })).toBeTruthy();

    fireEvent.click(screen.getByText("模型"));
    expect(screen.getByRole("button", { name: "Agent 默认" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "One Time Model" }));

    fireEvent.click(screen.getByText("推理等级"));
    expect(screen.getByRole("button", { name: "Agent 默认" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "high" }));

    expect(runtimeBtn.textContent).toContain("high");

    const expected = { model: "one-time-model", provider: null, effort: "high" };
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalledWith("codex", [], "/tmp/workspace", expected));

    // The choice survives a restart even though nothing was launched yet.
    cleanup();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", expected));
    expect(screen.getByRole("button", { name: "模型与推理设置" }).textContent).toContain("high");

    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始工作" } });
    await waitFor(() => expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", [], "/tmp/workspace", "开始工作", expected));
    cleanup();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", expected));
    expect(screen.getByRole("button", { name: "模型与推理设置" }).textContent).toContain("high");
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "pi" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("pi", [], "/tmp/workspace", NATIVE_RUNTIME));
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "codex" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", expected));

    // The latest choice stays remembered even when its launch fails.
    fireEvent.click(screen.getByRole("button", { name: "模型与推理设置" }));
    fireEvent.click(screen.getByText("模型"));
    fireEvent.click(await screen.findByRole("button", { name: "Agent 默认" }));
    vi.mocked(api.launchEmbeddedNew).mockRejectedValue(new Error("launch failed"));
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "重试" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).not.toHaveProperty("disabled", true));
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await screen.findByText(/launch failed/);
    cleanup();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/tmp/workspace", NATIVE_RUNTIME));
  });

  it("excludes agents when detected or launch_enabled is false", async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue({
      ...mockAgentStatus,
      codex: { ...mockAgentStatus.codex, detected: false },
      pi: { ...mockAgentStatus.pi, launch_enabled: false },
    });
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code"));
    expect(screen.queryByRole("option", { name: "Codex" })).toBeNull();
    expect(screen.queryByRole("option", { name: "Pi" })).toBeNull();
    expect(screen.getByRole("option", { name: "Claude Code" })).toBeTruthy();
  });

  it("clears one-time parameters on an agent switch and prevents launching when all are disabled", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockImplementation(async (agent) => ({
      ...runtimeFixture(agent),
      models: [{ id: "codex-only", display_name: "Codex Only", provider: null, supported_efforts: ["low", "high"] }],
    }));
    render(<NewSessionView navigate={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "模型与推理设置" }));
    fireEvent.click(screen.getByText("模型"));
    fireEvent.click(await screen.findByRole("button", { name: "Codex Only" }));

    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalledWith("codex", [], "/tmp/workspace", { model: "codex-only", provider: null, effort: null }));

    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "pi" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("pi", [], "/tmp/workspace", NATIVE_RUNTIME));
    cleanup();
    vi.mocked(api.getAgentStatus).mockResolvedValue(Object.fromEntries(Object.entries(mockAgentStatus).map(([agent, entry]) => [agent, { ...entry, detected: false }])));
    render(<NewSessionView navigate={vi.fn()} />);
    await screen.findByText("没有可用的新建会话 Agent，请确保本地已安装支持的 Agent CLI。");
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始" } });
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true);
    expect(api.launchEmbeddedNew).not.toHaveBeenCalled();
  });
});

it("launches the assistant project using the shared project and task selection", async () => {
  const navigate = vi.fn();
  const directory = "/tmp/custom-home/assistant";
  const assistantProject: ProjectCardData = {
    ...chatProject, id: "assistant-project", name: "我的助手", kind: "single_directory", purpose: "assistant",
    search_paths: [directory], representative_paths: [directory],
  };
  vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, assistantProject]);
  vi.mocked(api.getDefaultAgent).mockResolvedValue("antigravity");
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "assistant-terminal", note: "", command_line: "", launched_via: "embedded" });
  render(<NewSessionView navigate={navigate} projectId="assistant-project" />);
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("antigravity", [], directory, NATIVE_RUNTIME));
  expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("assistant-project");
  expect(screen.getByLabelText("跟踪任务（可选）")).toBeTruthy();
  expect([...screen.getByLabelText("Agent").querySelectorAll("option")].map(o => o.value)).toEqual(["codex", "claude_code", "pi", "antigravity"]);
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "查询这个项目的进展" } });
  await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).not.toHaveProperty("disabled", true));
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "发送" })); });
  expect(api.launchEmbeddedNew).toHaveBeenCalledWith("antigravity", [], directory, "查询这个项目的进展", NATIVE_RUNTIME);
  expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "assistant-terminal" });
});

describe("NewSessionView saved choices and drafts", () => {
  const project: ProjectCardData = {
    ...chatProject, id: "repo", name: "代码项目", kind: "git",
    search_paths: ["/repo/main", "/repo/worktree"], representative_paths: ["/repo/main"],
  };

  async function chooseProjectAndDirectory() {
    await screen.findByRole("option", { name: "代码项目" });
    fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "repo" } });
    fireEvent.change(screen.getByLabelText("项目目录"), { target: { value: "/repo/worktree" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", [], "/repo/worktree", NATIVE_RUNTIME));
  }

  function restart(props: Partial<React.ComponentProps<typeof NewSessionView>> = {}) {
    cleanup();
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} {...props} />);
  }

  it("restores manual choices and project drafts, and remembers entry presets", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, project]);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([
      { id: "task", title: "开发任务", visibility: "normal", projects: [{ id: "repo", name: "代码项目" }] },
    ] as WorkstreamCardData[]);
    render(<NewSessionView navigate={vi.fn()} />);
    await chooseProjectAndDirectory();
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "pi" } });
    openTaskMenu();
    fireEvent.click(screen.getByRole("checkbox", { name: "开发任务" }));
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "代码草稿\n保留换行" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("pi", ["task"], "/repo/worktree", NATIVE_RUNTIME));

    restart();
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("pi", ["task"], "/repo/worktree", NATIVE_RUNTIME));
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "代码草稿\n保留换行");
    openTaskMenu();
    expect(screen.getByRole("checkbox", { name: "开发任务" })).toHaveProperty("checked", true);

    restart({ projectId: "chat", agent: "claude_code" });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", [], "/tmp/workspace", NATIVE_RUNTIME));
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "");
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "聊天草稿" } });

    // Project and Agent presets become the choices remembered without a preset.
    restart();
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", [], "/tmp/workspace", NATIVE_RUNTIME));
    expect(screen.getByLabelText("所属项目")).toHaveProperty("value", "chat");
    expect(screen.getByLabelText("Agent")).toHaveProperty("value", "claude_code");
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "聊天草稿");

    await act(async () => { fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "repo" } }); });
    expect(screen.getByLabelText("项目目录")).toHaveProperty("value", "/repo/worktree");
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "代码草稿\n保留换行");
    openTaskMenu();
    expect(screen.getByRole("checkbox", { name: "开发任务" })).toHaveProperty("checked", false);
    expect(api.setDefaultAgent).not.toHaveBeenCalled();
  });

  it("falls back when saved choices become unavailable", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, project]);
    render(<NewSessionView navigate={vi.fn()} />);
    await chooseProjectAndDirectory();
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "pi" } });

    vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, { ...project, search_paths: ["/repo/main"] }]);
    vi.mocked(api.getAgentStatus).mockResolvedValue({ ...mockAgentStatus, pi: { ...mockAgentStatus.pi, launch_enabled: false } });
    vi.mocked(api.getDefaultAgent).mockResolvedValue("claude_code");
    restart();
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", [], "/repo/main", NATIVE_RUNTIME));

    vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, { ...project, visibility: "archived" }]);
    restart();
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", [], "/tmp/workspace", NATIVE_RUNTIME));
    expect(screen.queryByRole("option", { name: "代码项目" })).toBeNull();
  });

  it("preserves the draft on launch failure and clears it after a successful launch even after leaving", async () => {
    vi.mocked(api.launchEmbeddedNew).mockRejectedValueOnce(new Error("launch failed"));
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalled());
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "待发送内容" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await screen.findByText("启动失败：Error: launch failed");

    restart();
    await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).toHaveProperty("disabled", false));
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "待发送内容");
    let finish!: (result: Awaited<ReturnType<typeof api.launchEmbeddedNew>>) => void;
    vi.mocked(api.launchEmbeddedNew).mockReturnValue(new Promise(resolve => { finish = resolve; }));
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    cleanup();
    await act(async () => { finish({ terminal_id: "done", note: "", command_line: "", launched_via: "embedded" }); });
    viewState.clear();
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect(screen.getByLabelText("所属项目")).toHaveProperty("value", "chat"));
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "");
  });

  it("preserves text entered before project loading completes", async () => {
    let loadProjects!: (projects: ProjectCardData[]) => void;
    vi.mocked(api.listProjectCards).mockReturnValueOnce(new Promise(resolve => { loadProjects = resolve; }));
    render(<NewSessionView navigate={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "加载期间的草稿" } });
    await act(async () => { loadProjects([chatProject]); });
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "加载期间的草稿");
    restart();
    await waitFor(() => expect(screen.getByLabelText("所属项目")).toHaveProperty("value", "chat"));
    expect(screen.getByLabelText("首条消息")).toHaveProperty("value", "加载期间的草稿");
  });
});


it("launches with multiple checked tasks and keeps only shared tasks after changing projects", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([chatProject, { ...chatProject, id: "repo", name: "代码项目", kind: "git", search_paths: ["/repo/main"], representative_paths: ["/repo/main"] }]);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([
    { id: "first", title: "界面优化", visibility: "normal", projects: [{ id: "chat", name: "NoEnding Chat" }] },
    { id: "shared", title: "发布检查", visibility: "normal", projects: [{ id: "chat", name: "NoEnding Chat" }, { id: "repo", name: "代码项目" }] },
    { id: "archived", title: "旧任务", visibility: "archived", projects: [{ id: "chat", name: "NoEnding Chat" }] },
  ] as WorkstreamCardData[]);
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "multi-task-terminal", note: "", command_line: "", launched_via: "embedded" });
  render(<NewSessionView navigate={vi.fn()} />);
  openTaskMenu();
  fireEvent.click(await screen.findByRole("checkbox", { name: "界面优化" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "发布检查" }));
  expect(screen.queryByRole("checkbox", { name: "旧任务" })).toBeNull();
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", ["first", "shared"], "/tmp/workspace", NATIVE_RUNTIME));
  fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "repo" } });
  openTaskMenu();
  expect(screen.queryByRole("checkbox", { name: "界面优化" })).toBeNull();
  expect(screen.getByRole("checkbox", { name: "发布检查" })).toHaveProperty("checked", true);
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", ["shared"], "/repo/main", NATIVE_RUNTIME));
  fireEvent.change(screen.getByLabelText("所属项目"), { target: { value: "chat" } });
  openTaskMenu();
  fireEvent.click(screen.getByRole("checkbox", { name: "界面优化" }));
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始这两项工作" } });
  await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).not.toHaveProperty("disabled", true));
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", ["shared", "first"], "/tmp/workspace", "开始这两项工作", NATIVE_RUNTIME));
});

it("shows chat projects including NoEnding Chat and agent chat, but filters all-missing projects", async () => {
  const otherChatProj: ProjectCardData = { ...chatProject, id: "agent-chat", name: "WorkBuddy Chat", purpose: "chat" as const, search_paths: ["/tmp/workbuddy-chat"], representative_paths: ["/tmp/workbuddy-chat"] };
  const missingProj: ProjectCardData = { ...chatProject, id: "missing", name: "丢失项目", path_count: 1, missing_path_count: 1, search_paths: ["/tmp/missing"], representative_paths: ["/tmp/missing"] };
  const validProj: ProjectCardData = { ...chatProject, id: "valid", name: "有效项目", purpose: null as never, search_paths: ["/tmp/workspace"], representative_paths: ["/tmp/workspace"] };
  const noEndingChatProj: ProjectCardData = { ...chatProject, id: "noending-chat", name: "NoEnding Chat", purpose: "chat" as const, search_paths: ["/tmp/noending"], representative_paths: ["/tmp/noending"] };
  vi.mocked(api.listProjectCards).mockResolvedValue([otherChatProj, missingProj, validProj, noEndingChatProj]);
  render(<NewSessionView navigate={vi.fn()} />);
  await screen.findByRole("option", { name: "有效项目" });
  expect(screen.getByRole("option", { name: "NoEnding Chat" })).toBeTruthy();
  expect(screen.getByRole("option", { name: "WorkBuddy Chat" })).toBeTruthy();
  expect(screen.queryByRole("option", { name: "丢失项目" })).toBeNull();
});

