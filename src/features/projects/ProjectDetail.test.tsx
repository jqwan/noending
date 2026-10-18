import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectDetail, { clearProjectDetailCache } from "./ProjectDetail";
import { api } from "../../api";
import type { ExtensionInventory, ProjectDetailData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    listPreferences: vi.fn().mockResolvedValue([]),
    savePreference: vi.fn(),
    deletePreference: vi.fn(),
    getProjectDetail: vi.fn(),
    listExtensionResources: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    listExtensionCatalog: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    enableExtensionSkillProjects: vi.fn(),
    disableExtensionSkillProjects: vi.fn(),
    enableMcpDefinitionProjects: vi.fn(),
    disableMcpDefinitionProjects: vi.fn(),
    listProjectCards: vi.fn(),
    createWorkstream: vi.fn(),
    renameProject: vi.fn(),
    getProjectContextState: vi.fn(),
    updateProjectContext: vi.fn(),
    regenerateProjectContext: vi.fn(),
    getAgentStatus: vi.fn(),
    continueSessionDesktop: vi.fn(),
  },
}));

const navigate = vi.fn();

beforeEach(() => {
  clearProjectDetailCache();
  vi.mocked(api.listPreferences).mockReset().mockResolvedValue([]);
  navigate.mockReset();
  vi.mocked(api.getProjectContextState).mockReset().mockResolvedValue({project_id: "p1", summary: null, context_revision: 0, updated_at: null, pending: true, pending_sessions: 1});
  vi.mocked(api.updateProjectContext).mockReset();
  vi.mocked(api.regenerateProjectContext).mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.createWorkstream).mockReset();
  vi.mocked(api.getAgentStatus).mockResolvedValue({
    codex: {
      name: "codex",
      detected: true,
      executable: "/bin/codex",
      version: "1.0.0",
      terminal_cli: true,
      launch_enabled: true,
      desktop_app: "Codex",
      desktop_app_present: true,
    },
  });
  vi.mocked(api.continueSessionDesktop).mockResolvedValue({
    uri: "vscode://...",
    note: "已在桌面应用中打开",
  });
});

afterEach(cleanup);

function mockDetail(): ProjectDetailData {
  return {
    kind: "git",
    project: {
      id: "p1",
      name: "Test Project",
      git_id: "git-1",
      name_customized: false, visibility: "normal",
      created_at: "2026-10-01T00:00:00Z",
      updated_at: "2026-10-01T00:00:00Z",
    },
    workspace_paths: [
      {
        id: "wp-linked",
        canonical_path: "/a/linked/worktree",
        project_id: "p1",
        git_state: "detected",
        git_kind: "linked",
        exists: true,
        first_seen_at: "2026-10-01T00:00:00Z",
        last_seen_at: "2026-10-01T00:00:00Z",
      },
      {
        id: "wp-main",
        canonical_path: "/z/main/worktree",
        project_id: "p1",
        git_state: "detected",
        git_kind: "main",
        exists: true,
        first_seen_at: "2026-10-01T00:00:00Z",
        last_seen_at: "2026-10-01T00:00:00Z",
      },
    ],
    workstreams: [],
    sessions: [
      {
        id: "s1",
        agent: "codex",
        root_agent_session_id: "root-1",
        title: "Session 1",
        cwd: "/z/main/worktree",
        project_id: "p1",
        workspace_path_id: "wp-main",
        tracked_workstream_ids: [],
        started_at: "2026-10-01T00:00:00Z",
        last_activity_at: "2026-10-01T01:00:00Z",
        last_conversation_at: null,
        archived_at: null,
        source_kind: "codex_rollout",
        latest_message_seq: 1,
      },
    ],
    remote_url: null,
  };
}

describe("ProjectDetail", () => {
  it("keeps project type and assistant purpose separate", async () => {
    const detail = mockDetail();
    detail.kind = "git";
    detail.purpose = "assistant";
    detail.workspace_paths[0].exists = false;
    vi.mocked(api.getProjectDetail).mockResolvedValue(detail);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    const newSessionBtn = screen.getByRole("button", { name: "新建会话" });
    expect(newSessionBtn).toBeTruthy();

    fireEvent.click(newSessionBtn);
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p1" });
    expect(screen.getByText("Git 项目")).toBeTruthy();
    expect(screen.getByText("助手")).toBeTruthy();
  });

  it("does not offer new sessions in chat-directory projects", async () => {
    const agentDetail = mockDetail();
    agentDetail.kind = "directory_group";
    agentDetail.project.name = "Codex Chat";
    vi.mocked(api.getProjectDetail).mockResolvedValue(agentDetail);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Codex Chat");
    expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  });

  it("navigates without defaulting project for new sessions when directories are missing", async () => {
    const d = mockDetail();
    d.workspace_paths = d.workspace_paths.map(path => ({ ...path, exists: false }));
    vi.mocked(api.getProjectDetail).mockResolvedValue(d);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const newSession = await screen.findByRole("button", { name: "新建会话" });
    fireEvent.click(newSession);
    expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  });

  it("creates a task in this project and refreshes its associated task list", async () => {
    const d = mockDetail();
    // A missing directory does not block creation while another still exists.
    d.workspace_paths[0].exists = false;
    vi.mocked(api.listProjectCards).mockResolvedValue([{
      id: "p1", name: "Test Project", kind: "single_directory", visibility: "normal",
      path_count: 2, missing_path_count: 1, search_paths: ["/z/main/worktree"], representative_paths: ["/z/main/worktree"],
      name_customized: false, workstream_count: 0, session_count: 0,
      last_activity_at: null, updated_at: "",
    }]);
    vi.mocked(api.getProjectDetail).mockImplementation(async () => ({ ...d }));
    vi.mocked(api.createWorkstream).mockImplementation(async () => {
      const task = { id: "task-created", title: "从项目新建的任务", description: "",
        visibility: "normal" as const, created_at: "", updated_at: "" };
      d.workstreams = [{ workstream: task }];
      return task;
    });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    fireEvent.click(await screen.findByRole("button", { name: "新建任务" }));
    expect(screen.getByRole("dialog", { name: "新建任务" })).toBeTruthy();
    await screen.findByRole("button", { name: "移除项目 Test Project" });
    fireEvent.change(screen.getByRole("textbox", { name: "标题" }), { target: { value: "从项目新建的任务" } });
    fireEvent.submit(screen.getByRole("textbox", { name: "标题" }).closest("form")!);
    await waitFor(() => expect(api.createWorkstream).toHaveBeenCalledWith("从项目新建的任务", "", ["p1"]));
    const taskLink = await screen.findByRole("link", { name: "从项目新建的任务" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("heading", { name: "关联任务 (1)" })).toBeTruthy();
    fireEvent.click(taskLink);
    expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "task-created", initialTitle: "从项目新建的任务" });
  });

  it("orders git main worktree first in workspace paths list", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    // Even though /a/linked/worktree is alphabetically earlier, /z/main/worktree is git_kind: "main" and comes first
    expect(screen.getAllByTitle(/\/worktree$/).map(link => link.getAttribute("title")))
      .toEqual(["/z/main/worktree", "/a/linked/worktree"]);
  });

  it("renders desktop continue for desktop-only session rows and stops row navigation", async () => {
    const d = mockDetail();
    d.sessions[0].agent = "workbuddy";
    vi.mocked(api.getProjectDetail).mockResolvedValue(d);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Session 1");
    const continueBtn = screen.getByRole("button", { name: /在桌面应用中继续Session 1/ });
    expect(continueBtn).toBeTruthy();

    fireEvent.click(continueBtn);
    expect(api.continueSessionDesktop).toHaveBeenCalledWith("s1");
    // Row click should NOT have navigated to session detail
    expect(navigate).not.toHaveBeenCalled();
  });

  it("reads Markdown without extracting, explicitly updates with partial feedback, and regenerates separately", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    vi.mocked(api.getProjectContextState).mockResolvedValue({project_id: "p1", summary: "**当前结论**", context_revision: 1, updated_at: null, pending: true, pending_sessions: 1});
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const conclusion = await screen.findByText("当前结论");
    expect(api.listPreferences).toHaveBeenCalledWith("p1");
    expect(conclusion.tagName).toBe("STRONG");
    expect(api.updateProjectContext).not.toHaveBeenCalled();
    expect(api.regenerateProjectContext).not.toHaveBeenCalled();
    vi.mocked(api.getProjectContextState).mockResolvedValue({project_id: "p1", summary: "更新后的项目结论", context_revision: 2, updated_at: null, pending: true, pending_sessions: 1});
    vi.mocked(api.updateProjectContext).mockResolvedValue({project_id: "p1", status: "partial", context_revision: 2, updated_sessions: [], skipped_session_ids: [], failed_sessions: [{session_id: "s1", error: "读取失败"}], remaining_pending: 1, preference_error: "Agent 不能修改锁定偏好"});
    fireEvent.click(screen.getByRole("button", {name: "更新摘要"}));
    await screen.findByText("更新后的项目结论");
    expect(api.updateProjectContext).toHaveBeenCalledWith("p1");
    expect(screen.getAllByRole("alert").some(el => el.textContent?.includes("Session 1：读取失败"))).toBe(true);
    expect(screen.getByText(/偏好未保存：Agent 不能修改锁定偏好/)).toBeTruthy();
    vi.mocked(api.getProjectContextState).mockResolvedValue({project_id: "p1", summary: "重新生成的结论", context_revision: 3, updated_at: null, pending: false, pending_sessions: 0});
    vi.mocked(api.regenerateProjectContext).mockResolvedValue({project_id: "p1", status: "updated", context_revision: 3, updated_sessions: ["s1"], skipped_session_ids: [], failed_sessions: [], remaining_pending: 0, preference_error: null});
    fireEvent.click(screen.getByRole("button", {name: "重新生成"}));
    await screen.findByText("重新生成的结论");
    expect(api.regenerateProjectContext).toHaveBeenCalledWith("p1");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", {name: "更新摘要"})).toBeNull();
  });

  it("adds extensions to this project from the card", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [],
      resources: [
        { id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: [], entry_paths: [], entry_links: [], revision: "r", registered: true, error: null },
        { id: "lint", kind: "skill" as const, name: "lint", description: "Lint", path: "/home/test/.agents/skills/lint/SKILL.md", locations: [], entry_paths: [], entry_links: [], revision: "r", registered: true, error: null },
        { id: "mcp-docs", kind: "mcp" as const, name: "docs", description: "Docs", path: "", locations: [], entry_paths: [], entry_links: [], revision: "r", registered: true, error: null },
      ],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockResolvedValue({ locations: [], resources: [], warnings: [] });
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(inventory);
    vi.mocked(api.enableExtensionSkillProjects).mockClear().mockResolvedValue({ message: "Enabled" });
    vi.mocked(api.enableMcpDefinitionProjects).mockClear().mockResolvedValue({ message: "Enabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    expect(await within(section).findByText("当前项目还没有启用的扩展。")).toBeTruthy();
    fireEvent.click(within(section).getByRole("button", { name: "配置扩展" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /lint/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /docs/ }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.enableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "lint"));
    await waitFor(() => expect(api.enableMcpDefinitionProjects).toHaveBeenCalledWith(["p1"], "mcp-docs"));
    expect(api.enableExtensionSkillProjects).toHaveBeenCalledTimes(1);
  });

  it("opens an extension detail from the project card", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [{ id: "shared", kind: "skill" as const, agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(inventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    fireEvent.click(within(section).getByRole("button", { name: /review/ }));
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extension", resourceId: "review", kind: "skill" }));
  });

  it("removes a project extension by unchecking it in the dialog", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [{ id: "shared", kind: "skill" as const, agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(inventory);
    vi.mocked(api.disableExtensionSkillProjects).mockClear().mockResolvedValue({ message: "Disabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    fireEvent.click(await within(section).findByRole("button", { name: "配置扩展" }));
    const checkbox = await screen.findByRole("checkbox", { name: /review/ }) as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    fireEvent.click(checkbox);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.disableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
  });

  it("keeps the project extension list visible and other rows interactive after an action", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [{ id: "shared", kind: "skill" as const, agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [
        { id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null },
        { id: "lint", kind: "skill" as const, name: "lint", description: "Lint", path: "/home/test/.agents/skills/lint/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/lint/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null },
      ],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    let finish!: (value: ExtensionInventory) => void;
    vi.mocked(api.listExtensionCatalog).mockClear()
      .mockResolvedValueOnce(inventory)
      .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    vi.mocked(api.enableExtensionSkillProjects).mockClear().mockResolvedValue({ message: "Enabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    expect(within(section).getByRole("heading", { name: "扩展 (2)" })).toBeTruthy();
    fireEvent.click((await within(section).findAllByRole("button", { name: "补齐" }))[0]);
    await waitFor(() => expect(api.enableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
    await waitFor(() => expect(api.listExtensionCatalog).toHaveBeenCalledTimes(2));
    expect(within(section).queryByText("正在读取扩展配置…")).toBeNull();
    const buttons = within(section).getAllByRole("button", { name: "补齐" }) as HTMLButtonElement[];
    expect(buttons[0].disabled).toBe(true);
    expect(buttons[1].disabled).toBe(false);
    await act(async () => { finish(inventory); });
  });

  it("keeps a user-level project Skill read-only", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const userInventory: ExtensionInventory = {
      locations: [{ id: "u", kind: "skill" as const, agent: null, scope: "user" as const, path: "/home/test/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["u"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? { locations: [], resources: [], warnings: [] }
      : userInventory);
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(userInventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    const row = await within(section).findByRole("button", { name: /review/ });
    expect(within(row).queryByRole("button", { name: /^(补齐|停用)$/ })).toBeNull();
    fireEvent.click(within(section).getByRole("button", { name: "配置扩展" }));
    const fixed = await screen.findByRole("checkbox", { name: /review/ }) as HTMLInputElement;
    expect(fixed.checked).toBe(true);
    expect(fixed.disabled).toBe(true);
  });

  it("keeps an unregistered project Skill read-only", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [{ id: "shared", kind: "skill" as const, agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", kind: "skill" as const, name: "review", description: "Review", path: "/home/test/external/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: false, error: null }],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(inventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    expect(await within(section).findByText("未托管")).toBeTruthy();
    expect(within(section).queryByRole("button", { name: /^(补齐|停用)$/ })).toBeNull();
  });

  it("lists an enabled MCP definition for this project", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: ExtensionInventory = {
      locations: [{ id: "mcp-project", kind: "mcp" as const, agent: "claude_code", scope: "project" as const, path: "/z/main/worktree/.mcp.json", label: "项目 MCP" }],
      resources: [{ id: "mcp-docs", kind: "mcp" as const, name: "docs", description: "", path: "", locations: ["mcp-project"], entry_paths: ["/z/main/worktree/.mcp.json"], entry_links: [], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listExtensionResources).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listExtensionCatalog).mockResolvedValue(inventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("扩展");
    expect(await within(section).findByText("docs")).toBeTruthy();
  });

  it("retains the current summary on extraction failure and does not show an old project's late result", async () => {
    vi.mocked(api.getProjectDetail).mockImplementation(async id => ({...mockDetail(), project: {...mockDetail().project, id, name: id}}));
    vi.mocked(api.getProjectContextState).mockImplementation(async id => ({project_id: id, summary: `${id} 当前结论`, context_revision: 1, updated_at: null, pending: true, pending_sessions: 1}));
    vi.mocked(api.updateProjectContext).mockRejectedValueOnce("提取失败");
    const {rerender} = render(<ProjectDetail projectId="p1" navigate={navigate} />);
    await screen.findByText("p1 当前结论");
    fireEvent.click(screen.getByRole("button", {name: "更新摘要"}));
    await screen.findByRole("alert");
    expect(screen.getByText("p1 当前结论")).toBeTruthy();
    let finish!: (value: Awaited<ReturnType<typeof api.updateProjectContext>>) => void;
    vi.mocked(api.updateProjectContext).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    fireEvent.click(screen.getByRole("button", {name: "更新摘要"}));
    await waitFor(() => expect(api.updateProjectContext).toHaveBeenCalledTimes(2));
    rerender(<ProjectDetail projectId="p2" navigate={navigate} />);
    await screen.findByText("p2 当前结论");
    finish({project_id: "p1", status: "partial", context_revision: 2, updated_sessions: [], skipped_session_ids: [], failed_sessions: [{session_id: "s1", error: "旧项目结果"}], remaining_pending: 1, preference_error: null});
    await waitFor(() => expect(screen.queryByText(/旧项目结果/)).toBeNull());
    expect(screen.getByText("p2 当前结论")).toBeTruthy();
  });

});
