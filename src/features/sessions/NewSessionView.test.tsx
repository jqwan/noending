import { clearRuntimeCatalog } from "../agents/useAgentRuntime";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runtimeFixture } from "../agents/runtimeFixtures";
import NewSessionView from "./NewSessionView";
import { api } from "../../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { PathProbe, ProjectCardData, WorkstreamCardData } from "../../types";

const probeFixture: PathProbe = {raw: "/tmp/workspace", status: "ok", canonical_path: "/tmp/workspace", exists: true, git_state: "none", git_kind: null, project: null};

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
    setAgentRuntimeOverrides: vi.fn(),
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
  initial_workstream_id: null,
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
  clearRuntimeCatalog();
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async (agent) => runtimeFixture(agent));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockRejectedValue(new Error("catalog unavailable"));
  vi.mocked(api.setAgentRuntimeOverrides).mockReset();
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
    initial_workstream_id: ownerId,
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
      initial_workstream_id: null,
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
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", null, "/tmp/workspace");
    });

    expect(api.setDefaultAgent).not.toHaveBeenCalled();
    const select = screen.getByLabelText("Agent") as HTMLSelectElement;
    expect(select.value).toBe("claude_code");
  });

  it("falls back to default agent when no agent is specified", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/tmp/workspace");
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
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("claude_code", null, "/tmp/workspace"));
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
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/tmp/workspace");
    });

    const select = screen.getByLabelText("Agent");
    fireEvent.change(select, { target: { value: "claude_code" } });

    await waitFor(() => {
      expect(api.prepareNewSession).toHaveBeenCalledWith("claude_code", null, "/tmp/workspace");
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


  it("renders + 新任务 in task options and selecting it opens the new task modal", async () => {
    render(<NewSessionView navigate={vi.fn()} />);

    await waitFor(() => expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
    const taskSelect = (await screen.findByLabelText("跟踪任务（可选）")) as HTMLSelectElement;
    fireEvent.change(taskSelect, { target: { value: "__new_workstream__" } });

    expect(await screen.findByRole("heading", { name: "新建任务" })).toBeTruthy();
  });

  it("does not select a newly created task when the preset project association was removed", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue(mockProjects);
    const created = {id:"removed",title:"独立任务",description:"",visibility:"normal" as const,created_at:"",updated_at:""};
    vi.mocked(api.createWorkstream).mockResolvedValue(created);
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([]);
    render(<NewSessionView projectId="p-1" navigate={vi.fn()}/>);
    fireEvent.change(await screen.findByLabelText("跟踪任务（可选）"), {target:{value:"__new_workstream__"}});
    fireEvent.click(await screen.findByRole("button", {name:"移除项目 Project 1"}));
    fireEvent.change(screen.getByRole("textbox", {name:"标题"}), {target:{value:"独立任务"}});
    vi.mocked(api.listWorkstreamCards).mockResolvedValue([{...created,projects:[],summary:null,last_activity_at:null,session_count:0,latest_session:null}]);
    fireEvent.submit(screen.getByRole("textbox", {name:"标题"}).closest("form")!);
    await waitFor(()=>expect(api.createWorkstream).toHaveBeenCalledWith("独立任务","",[]));
    await waitFor(()=>expect(screen.queryByRole("dialog")).toBeNull());
    expect((screen.getByLabelText("跟踪任务（可选）") as HTMLSelectElement).value).toBe("none");
    expect(screen.queryByRole("option", {name:"独立任务"})).toBeNull();
  });

  it.each(["Codex Chat", "NoEnding Chat", "用户改名的聊天项目"])("includes chat-directory project %s from selection", async (name) => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      ...mockProjects,
      {
        id: "p-codex",
        name,
        name_customized: false, visibility: "normal" as const,
        kind: "directory_group" as const,
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
    expect(options).toContain(name);
  });

  it.each(["directory_group", "git"] as const)("defaults by workspace identity even for a renamed %s project", async (kind) => {
    const renamed = { ...chatProject, name: "我的聊天空间", name_customized: true, kind,
      search_paths: ["/tmp/workspace/another", "/tmp/workspace"] };
    vi.mocked(api.listProjectCards).mockResolvedValue([mockProjects[0], renamed]);
    render(<NewSessionView navigate={vi.fn()} />);
    const project = screen.getByLabelText("所属项目") as HTMLSelectElement;
    await waitFor(() => expect(project.value).toBe("chat"));
    expect(screen.queryByRole("option", { name: "无项目" })).toBeNull();
    expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", null, "/tmp/workspace"));
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
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", null, "/tmp/workspace"));
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
    await screen.findByRole("option", {name:"Multi Project"});
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
        summary: null,
        last_activity_at: null,
        session_count: 1,
        latest_session: null,


      },
    ]);

    render(<NewSessionView workstreamId="ws-1" navigate={vi.fn()} />);

    const projectSelect = (await screen.findByLabelText("所属项目")) as HTMLSelectElement;

    await waitFor(() => {
      expect(projectSelect.value).toBe("p-1");
      expect(api.prepareNewSession).toHaveBeenCalledWith("codex", "ws-1", "/repo/project-1");
    });

    fireEvent.change(projectSelect, { target: { value: "chat" } });
    await waitFor(() => {
      expect((screen.getByLabelText("项目目录") as HTMLSelectElement).value).toBe("/tmp/workspace");
      expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", null, "/tmp/workspace");
    });
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({
      launched_via: "内嵌终端", command_line: "codex", note: "已启动", terminal_id: "t-unassigned",
    });
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "继续这个任务" } });
    const send = screen.getByRole("button", { name: "发送" }) as HTMLButtonElement;
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, "/tmp/workspace", "继续这个任务"));
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
    expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, "/tmp/workspace", "请检查这段代码\n并修复问题"),
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
  const tasks=await screen.findByLabelText("跟踪任务（可选）");
  await waitFor(()=>expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
  expect(Array.from(tasks.querySelectorAll("option")).map(o=>o.value)).toEqual(["none","__new_workstream__"]);
  await act(async()=>{fireEvent.change(screen.getByLabelText("所属项目"), {target:{value:"p-1"}});});
  expect(Array.from(tasks.querySelectorAll("option")).map(o=>o.value)).toEqual(["none","matching","__new_workstream__"]);
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
  await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, "/tmp/workspace", "开始任务"));
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
  vi.mocked(api.listProjectCards).mockResolvedValue([chatProject,{id:"p-1",name:"甲",search_paths:["/repo/project-1"],representative_paths:["/repo/project-1"]}] as never);
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([{
    id: "multi", title: "跨项目任务", description: "", visibility: "normal" as const,
    projects: [{ id: "p-1", name: "甲" }, { id: "p-2", name: "乙" }],
    created_at: "", updated_at: "", summary: null,
    last_activity_at: null, session_count: 0, latest_session: null,
  }]);
  render(<NewSessionView workstreamId="multi" navigate={vi.fn()} />);
  const task = await screen.findByLabelText("跟踪任务（可选）");
  await waitFor(() => expect((screen.getByLabelText("所属项目") as HTMLSelectElement).value).toBe("chat"));
  expect((task as HTMLSelectElement).value).toBe("none");
  fireEvent.change(screen.getByLabelText("首条消息"), {target:{value:"开始"}});
  expect((screen.getByRole("button", {name:"发送"}) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("所属项目"), {target:{value:"p-1"}});
  fireEvent.change(task, {target:{value:"multi"}});
  await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("codex", "multi", "/repo/project-1"));
});

describe("NewSessionView runtime selection", () => {
  it("uses one-time runtime parameters for preview and launch without changing saved defaults", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockImplementation(async (agent) => ({
      ...runtimeFixture(agent),
      models: [{ id: "one-time-model", display_name: "One Time Model", provider: null, supported_efforts: ["low", "high"] }],
    }));
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "new-runtime", launched_via: "embedded", command_line: "test", note: "" });
    render(<NewSessionView navigate={vi.fn()} />);
    expect(screen.getByRole("heading", { name: "新建会话" })).toBeTruthy();

    const runtimeBtn = screen.getByRole("button", { name: "模型与推理设置" });
    await waitFor(() => expect(runtimeBtn.textContent).toContain("One Time Model"));
    expect(runtimeBtn.textContent).toContain("low");

    fireEvent.click(runtimeBtn);
    expect(await screen.findByRole("dialog", { name: "模型与推理设置" })).toBeTruthy();

    fireEvent.click(screen.getByText("模型"));
    expect(screen.queryByRole("button", { name: "Agent 默认" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "One Time Model" }));

    fireEvent.click(screen.getByText("推理等级"));
    expect(screen.queryByRole("button", { name: "Agent 默认" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "high" }));

    expect(runtimeBtn.textContent).toContain("high");

    const expected = { model: "one-time-model", provider: null, effort: "high" };
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/tmp/workspace", expected));
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始工作" } });
    await waitFor(() => expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, "/tmp/workspace", "开始工作", expected));
    expect(api.setAgentRuntimeOverrides).not.toHaveBeenCalled();
  });

  it("launches with agent native defaults when no overrides are selected", async () => {
    vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "native-runtime", launched_via: "embedded", command_line: "test", note: "" });
    render(<NewSessionView navigate={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始" } });
    await waitFor(() => expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(api.launchEmbeddedNew).toHaveBeenCalledWith("codex", null, "/tmp/workspace", "开始"));
    expect(api.setAgentRuntimeOverrides).not.toHaveBeenCalled();
  });

  it.each(["detected", "launch_enabled"] as const)("excludes agents when %s is false", async (field) => {
    vi.mocked(api.getAgentStatus).mockResolvedValue({ ...mockAgentStatus, codex: { ...mockAgentStatus.codex, [field]: false } });
    render(<NewSessionView navigate={vi.fn()} />);
    await waitFor(() => expect((screen.getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code"));
    expect(screen.queryByRole("option", { name: "Codex" })).toBeNull();
    expect(screen.getByRole("option", { name: "Pi" })).toBeTruthy();
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

    await waitFor(() => expect(api.prepareNewSession).toHaveBeenCalledWith("codex", null, "/tmp/workspace", { model: "codex-only", provider: null, effort: null }));

    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "pi" } });
    await waitFor(() => expect(api.prepareNewSession).toHaveBeenLastCalledWith("pi", null, "/tmp/workspace"));
    cleanup();
    vi.mocked(api.getAgentStatus).mockResolvedValue(Object.fromEntries(Object.entries(mockAgentStatus).map(([agent, entry]) => [agent, { ...entry, detected: false }])));
    render(<NewSessionView navigate={vi.fn()} />);
    await screen.findByText("没有可用的新建会话代理，请确保本地已安装支持的 Agent CLI。");
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "开始" } });
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true);
    expect(api.launchEmbeddedNew).not.toHaveBeenCalled();
  });
});

it("launches an ordinary assistant conversation without a separate target or agent exclusion", async () => {
  const navigate = vi.fn();
  const workspace: import("../../types").AssistantWorkspace = {
    project: { id: "assistant-project", name: "我的助手", git_id: null, name_customized: true, visibility: "normal" as const, created_at: "", updated_at: "" },
    directory: "/tmp/custom-home/assistant"
  };
  vi.mocked(api.listProjectCards).mockResolvedValue([{ id: "target-project", name: "真实项目", kind: "git", search_paths: ["/repo/target"], representative_paths: ["/repo/target"] }] as never);
  vi.mocked(api.getDefaultAgent).mockResolvedValue("antigravity");
  vi.mocked(api.launchEmbeddedNew).mockResolvedValue({ terminal_id: "assistant-terminal", note: "", command_line: "", launched_via: "embedded" });
  await act(async () => { render(<NewSessionView navigate={navigate} assistantWorkspace={workspace} />); });
  expect(screen.queryByLabelText("跟踪任务（可选）")).toBeNull();
  expect([...screen.getByLabelText("Agent").querySelectorAll("option")].map(o => o.value)).toEqual(["codex", "claude_code", "pi", "antigravity"]);
  expect(screen.queryByLabelText("助手目标范围")).toBeNull();
  expect(api.listProjectCards).not.toHaveBeenCalled();
  expect(api.listWorkstreamCards).not.toHaveBeenCalled();
  expect(api.prepareNewSession).toHaveBeenLastCalledWith("antigravity", null, workspace.directory);
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "查询这个项目的进展" } });
  await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).not.toHaveProperty("disabled", true));
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "发送" })); });
  expect(api.launchEmbeddedNew).toHaveBeenCalledWith("antigravity", null, workspace.directory, "查询这个项目的进展", undefined);
  expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "assistant-terminal", assistant: true });
});
