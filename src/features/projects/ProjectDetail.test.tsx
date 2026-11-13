import { clearReadResources } from "../../hooks/readResource";
import { refreshOperationStatus } from "../../hooks/useOperationState";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectDetail, { clearProjectDetailCache } from "./ProjectDetail";
import SkillScopeConfig from "../skills/SkillScopeConfig";
import { api } from "../../api";
import type { SkillInventory, ProjectDetailData, OperationStatus } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getOperationStatus: vi.fn().mockResolvedValue([]),
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    getProjectDetail: vi.fn(),
    listSkills: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    listSkillCatalog: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    enableSkillProjects: vi.fn(),
    disableSkillProjects: vi.fn(),
    listProjectCards: vi.fn(),
    createWorkstream: vi.fn(),
    renameProject: vi.fn(),
    cleanMissingProjectPaths: vi.fn(),
    getProjectContextState: vi.fn(),
    updateProjectContext: vi.fn(),
    regenerateProjectContext: vi.fn(),
    getAgentStatus: vi.fn(),
    continueSessionDesktop: vi.fn(),
  },
}));

const navigate = vi.fn();

it("keeps project skills visible on return and after a failed fresh read", async () => {
  const inventory: SkillInventory = {
    locations: [{ id: "shared", agent: null, scope: "project", path: "/project/.agents/skills", label: "共享 Skills" }],
    resources: [{ id: "review", name: "review", description: "", path: "/home/skills/review", registered: true, locations: ["shared"], entry_paths: ["/project/.agents/skills/review"], entry_links: [true], revision: "r1", error: null }],
    warnings: [],
  };
  vi.mocked(api.listSkills).mockResolvedValue(inventory);
  vi.mocked(api.listSkillCatalog).mockResolvedValue(inventory);
  const first = render(<SkillScopeConfig projectId="p1" navigate={navigate} />);
  await screen.findByRole("button", { name: "review" });
  first.unmount();
  vi.mocked(api.listSkills).mockRejectedValue(new Error("技能读取失败"));
  render(<SkillScopeConfig projectId="p1" navigate={navigate} />);
  expect(screen.getByRole("button", { name: "review" })).toBeTruthy();
  expect((await screen.findByRole("alert")).textContent).toContain("技能读取失败");
  expect(screen.getByRole("button", { name: "review" })).toBeTruthy();
});

beforeEach(() => {
  clearReadResources();
  clearProjectDetailCache();
  navigate.mockReset();
  vi.mocked(api.getOperationStatus).mockReset().mockResolvedValue([]);
  vi.mocked(api.listSkills).mockReset().mockResolvedValue({ resources: [], locations: [], warnings: [] });
  vi.mocked(api.listSkillCatalog).mockReset().mockResolvedValue({ resources: [], locations: [], warnings: [] });
  vi.mocked(api.getProjectContextState).mockReset().mockResolvedValue({project_id: "p1", summary: null, context_revision: 0, updated_at: null, pending: true, pending_sessions: 1, pending_session_summaries: 1, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },});
  vi.mocked(api.updateProjectContext).mockReset();
  vi.mocked(api.regenerateProjectContext).mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
  vi.mocked(api.listProjectCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.createWorkstream).mockReset();
  vi.mocked(api.cleanMissingProjectPaths).mockReset();
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
      },
      {
        id: "wp-main",
        canonical_path: "/z/main/worktree",
        project_id: "p1",
        git_state: "detected",
        git_kind: "main",
        exists: true,
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
    const taskLink = await screen.findByRole("button", { name: "从项目新建的任务" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("heading", { name: "关联任务（1）" })).toBeTruthy();
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

  it("shows every session newest first using actual time across offsets, with unknown times last", async () => {
    const d = mockDetail();
    const base = d.sessions[0];
    d.sessions = [
      { ...base, id: "unknown", title: "未知时间会话", started_at: null, last_activity_at: null },
      ...Array.from({ length: 12 }, (_, i) => ({ ...base, id: `dated-${i}`, title: `会话${i + 1}`, last_activity_at: `2026-10-${String(i + 1).padStart(2, "0")}T00:00:00Z` })),
      { ...base, id: "earlier", title: "跨时区较早会话", last_activity_at: "2026-10-29T01:30:00Z" },
      { ...base, id: "later", title: "跨时区较新会话", last_activity_at: "2026-10-29T10:00:00+08:00" },
    ];
    vi.mocked(api.getProjectDetail).mockResolvedValue(d);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const list = await screen.findByRole("region", { name: "会话列表" });
    expect(within(list).getAllByText(/^(会话\d+|跨时区.*会话|未知时间会话)$/).map(node => node.textContent)).toEqual([
      "跨时区较新会话", "跨时区较早会话", "会话12", "会话11", "会话10", "会话9", "会话8", "会话7", "会话6", "会话5", "会话4", "会话3", "会话2", "会话1", "未知时间会话",
    ]);
    fireEvent.click(within(list).getByRole("button", { name: /^未知时间会话/ }));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "unknown" }));
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

  it("reads structured summary topics without extracting, explicitly updates with partial feedback, and regenerates separately", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    vi.mocked(api.getProjectContextState).mockResolvedValue({project_id: "p1", summary: { items: [{ id: "ProjectDetail.test-item-1", title: "项目状态", description: "**当前结论**", details: [] }] }, context_revision: 1, updated_at: null, pending: true, pending_sessions: 1, pending_session_summaries: 1, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },});
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const conclusion = await screen.findByText("当前结论");
    screen.getByRole("heading", { name: "项目摘要（1）" });
    expect(conclusion.tagName).toBe("STRONG");
    expect(screen.getByRole("button", { name: "复制摘要" })).toBeTruthy();
    expect(api.updateProjectContext).not.toHaveBeenCalled();
    expect(api.regenerateProjectContext).not.toHaveBeenCalled();
    vi.mocked(api.updateProjectContext).mockResolvedValue({project_id: "p1", status: "partial", context_revision: 1, updated_sessions: [], skipped_session_ids: [], failed_sessions: [{session_id: "s1", error: "读取失败"}], remaining_pending: 1, remaining_session_summaries: 1,});
    fireEvent.click(screen.getByRole("button", {name: "更新摘要"}));
    await screen.findByText(/1 个会话待提取/);
    await screen.findByText(/Session 1：读取失败/);
    expect(screen.getByText("当前结论")).toBeTruthy();
    expect(api.updateProjectContext).toHaveBeenCalledWith("p1");
    expect(screen.getAllByRole("alert").some(el => el.textContent?.includes("Session 1：读取失败"))).toBe(true);
    vi.mocked(api.getProjectContextState).mockResolvedValue({project_id: "p1", summary: { items: [{ id: "ProjectDetail.test-item-3", title: "重新生成的结论", description: "", details: [] }] }, context_revision: 3, updated_at: null, pending: false, pending_sessions: 0, pending_session_summaries: 0, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },});
    vi.mocked(api.regenerateProjectContext).mockResolvedValue({project_id: "p1", status: "updated", context_revision: 3, updated_sessions: ["s1"], skipped_session_ids: [], failed_sessions: [], remaining_pending: 0, remaining_session_summaries: 0,});
    fireEvent.click(screen.getByRole("button", {name: "重新生成"}));
    await screen.findByText("重新生成的结论");
    expect(api.regenerateProjectContext).toHaveBeenCalledWith("p1");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", {name: "更新摘要"})).toBeNull();
  });

  it("shows live summary progress for the current project while preserving its existing summary", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const view = await api.getProjectContextState("p1");
    vi.mocked(api.getProjectContextState).mockResolvedValue({
      ...view,
      summary: { items: [{ id: "existing", title: "已有项目摘要", description: "", details: [] }] },
    });
    const operation: OperationStatus = {
      id: "project-update", kind: "summary", state: "running", agent: null,
      targets: [{ kind: "project", id: "p1" }],
      project_summary_progress: { processed: 0, remaining: 2, summary_saved: false },
    };
    vi.mocked(api.getOperationStatus).mockResolvedValue([
      { ...operation, id: "other-update", targets: [{ kind: "project", id: "other" }] },
      operation,
    ]);
    const first = render(<ProjectDetail projectId="p1" navigate={navigate} />);
    await screen.findByText("已处理 0 个会话，剩余 2 个");
    expect(screen.getByText("已有项目摘要")).toBeTruthy();

    operation.project_summary_progress = { processed: 1, remaining: 1, summary_saved: false };
    await act(() => refreshOperationStatus(true));
    await screen.findByText("已处理 1 个会话，剩余 1 个");

    first.unmount();
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    await screen.findByText("已处理 1 个会话，剩余 1 个");
    operation.project_summary_progress = { processed: 2, remaining: 0, summary_saved: false };
    await act(() => refreshOperationStatus(true));
    await screen.findByText("正在汇总项目摘要");

    vi.mocked(api.getProjectContextState).mockResolvedValue({
      ...view, pending: false, pending_session_summaries: 0, pending_sessions: 0,
      summary: { items: [{ id: "new", title: "新项目摘要", description: "", details: [] }] },
    });
    operation.project_summary_progress = { processed: 2, remaining: 0, summary_saved: true };
    await act(() => refreshOperationStatus(true));
    await screen.findByText("项目摘要已保存");
    await screen.findByText("新项目摘要");

    vi.mocked(api.getOperationStatus).mockResolvedValue([]);
    await act(() => refreshOperationStatus(true));
    expect(screen.queryByText("项目摘要已保存")).toBeNull();
    expect(screen.getByText("新项目摘要")).toBeTruthy();
  });

  it("adds skills to this project from the card", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: SkillInventory = {
      locations: [],
      resources: [
        { id: "review", name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: [], entry_paths: [], entry_links: [], revision: "r", registered: true, error: null },
        { id: "lint", name: "lint", description: "Lint", path: "/home/test/.agents/skills/lint/SKILL.md", locations: [], entry_paths: [], entry_links: [], revision: "r", registered: true, error: null },
      ],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockResolvedValue({ locations: [], resources: [], warnings: [] });
    vi.mocked(api.listSkillCatalog).mockResolvedValue(inventory);
    vi.mocked(api.enableSkillProjects).mockClear().mockResolvedValue({ message: "Enabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    expect(await within(section).findByText("当前项目还没有启用的技能。")).toBeTruthy();
    fireEvent.click(within(section).getByRole("button", { name: "配置技能" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /lint/ }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.enableSkillProjects).toHaveBeenCalledWith(["p1"], "lint"));
  });

  it("opens an skill detail from the project card", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: SkillInventory = {
      locations: [{ id: "shared", agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listSkillCatalog).mockResolvedValue(inventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    fireEvent.click(within(section).getByRole("button", { name: /review/ }));
    expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "skill", resourceId: "review" }));
  });

  it("removes a project skill by unchecking it in the dialog", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: SkillInventory = {
      locations: [{ id: "shared", agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listSkillCatalog).mockResolvedValue(inventory);
    vi.mocked(api.disableSkillProjects).mockClear().mockResolvedValue({ message: "Disabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    fireEvent.click(await within(section).findByRole("button", { name: "配置技能" }));
    const checkbox = await screen.findByRole("checkbox", { name: /review/ }) as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    fireEvent.click(checkbox);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.disableSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
  });

  it("keeps the project skill list visible and other rows interactive after an action", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: SkillInventory = {
      locations: [{ id: "shared", agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [
        { id: "review", name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null },
        { id: "lint", name: "lint", description: "Lint", path: "/home/test/.agents/skills/lint/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/lint/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null },
      ],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    let finish!: (value: SkillInventory) => void;
    vi.mocked(api.listSkillCatalog).mockClear()
      .mockResolvedValueOnce(inventory)
      .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    vi.mocked(api.enableSkillProjects).mockClear().mockResolvedValue({ message: "Enabled" });
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    expect(within(section).getByRole("heading", { name: "技能（2）" })).toBeTruthy();
    navigate.mockClear();
    fireEvent.click((await within(section).findAllByRole("button", { name: "补齐" }))[0]);
    await waitFor(() => expect(api.enableSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
    await waitFor(() => expect(api.listSkillCatalog).toHaveBeenCalledTimes(2));
    expect(within(section).queryByText("正在读取技能配置…")).toBeNull();
    const buttons = within(section).getAllByRole("button", { name: "补齐" }) as HTMLButtonElement[];
    expect(buttons[0].disabled).toBe(true);
    expect(buttons[1].disabled).toBe(false);
    await act(async () => { finish(inventory); });
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(within(section).getByRole("button", { name: "review" }));
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "skill", resourceId: "review" }));
  });

  it("keeps a user-level project Skill read-only", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const userInventory: SkillInventory = {
      locations: [{ id: "u", agent: null, scope: "user" as const, path: "/home/test/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", name: "review", description: "Review", path: "/home/test/.agents/skills/review/SKILL.md", locations: ["u"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? { locations: [], resources: [], warnings: [] }
      : userInventory);
    vi.mocked(api.listSkillCatalog).mockResolvedValue(userInventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    const row = await within(section).findByRole("button", { name: /review/ });
    expect(within(row).queryByRole("button", { name: /^(补齐|停用)$/ })).toBeNull();
    fireEvent.click(within(section).getByRole("button", { name: "配置技能" }));
    const fixed = await screen.findByRole("checkbox", { name: /review/ }) as HTMLInputElement;
    expect(fixed.checked).toBe(true);
    expect(fixed.disabled).toBe(true);
  });

  it("keeps a referenced project source enabled and unavailable for direct disable", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const userInventory: SkillInventory = {
      locations: [{ id: "u", agent: null, scope: "project" as const, path: "/z/main/worktree/.claude/skills", label: "共享 Skills" }],
      resources: [{ id: "review", name: "review", description: "Review", path: "/z/main/worktree/.claude/skills/review/SKILL.md", locations: ["u"], entry_paths: ["/z/main/worktree/.claude/skills/review/SKILL.md"], entry_links: [false], revision: "r", registered: true, error: null }],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? userInventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listSkillCatalog).mockResolvedValue(userInventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    const row = await within(section).findByRole("button", { name: /review/ });
    expect(within(row).queryByRole("button", { name: "停用" })).toBeNull();
    fireEvent.click(within(section).getByRole("button", { name: "配置技能" }));
    const fixed = await screen.findByRole("checkbox", { name: /review/ }) as HTMLInputElement;
    expect(fixed.checked).toBe(true);
    expect(fixed.disabled).toBe(true);
  });

  it("keeps an unregistered project Skill read-only", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const inventory: SkillInventory = {
      locations: [{ id: "shared", agent: null, scope: "project" as const, path: "/z/main/worktree/.agents/skills", label: "共享 Skills" }],
      resources: [{ id: "review", name: "review", description: "Review", path: "/home/test/external/review/SKILL.md", locations: ["shared"], entry_paths: ["/z/main/worktree/.agents/skills/review/SKILL.md"], entry_links: [true], revision: "r", registered: false, error: null }],
      warnings: [],
    };
    vi.mocked(api.listSkills).mockImplementation(async scope => scope.project_id
      ? inventory
      : { locations: [], resources: [], warnings: [] });
    vi.mocked(api.listSkillCatalog).mockResolvedValue(inventory);
    render(<ProjectDetail projectId="p1" navigate={navigate} />);
    const section = await screen.findByLabelText("技能");
    expect(await within(section).findByText("未托管")).toBeTruthy();
    expect(within(section).queryByRole("button", { name: /^(补齐|停用)$/ })).toBeNull();
  });


  it("retains the current summary on extraction failure and does not show an old project's late result", async () => {
    vi.mocked(api.getProjectDetail).mockImplementation(async id => ({...mockDetail(), project: {...mockDetail().project, id, name: id}}));
    vi.mocked(api.getProjectContextState).mockImplementation(async id => ({project_id: id, summary: { items: [{ id: `${id}-item`, title: `${id} 当前结论`, description: "", details: [] }] }, context_revision: 1, updated_at: null, pending: true, pending_sessions: 1, pending_session_summaries: 1, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },}));
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
    finish({project_id: "p1", status: "partial", context_revision: 1, updated_sessions: [], skipped_session_ids: [], failed_sessions: [{session_id: "s1", error: "旧项目结果"}], remaining_pending: 1, remaining_session_summaries: 1,});
    await waitFor(() => expect(screen.queryByText(/旧项目结果/)).toBeNull());
    expect(screen.getByText("p2 当前结论")).toBeTruthy();
  });

});

it("cleans missing unused directory registrations only after confirmation and refreshes the project", async () => {
  const detail = mockDetail();
  detail.workspace_paths[0].exists = false;
  vi.mocked(api.getProjectDetail).mockResolvedValue(detail);
  vi.mocked(api.cleanMissingProjectPaths).mockRejectedValueOnce(new Error("清理暂时失败"))
    .mockImplementationOnce(async () => {
      vi.mocked(api.getProjectDetail).mockResolvedValue({ ...detail, workspace_paths: [detail.workspace_paths[1]] });
      return 1;
    });
  render(<ProjectDetail projectId="p1" navigate={navigate} />);
  fireEvent.click(await screen.findByRole("button", { name: "清理失效目录" }));
  expect(screen.getByRole("dialog", { name: "清理失效目录？" }).textContent).toContain("1 个缺失且没有会话引用");
  expect(api.cleanMissingProjectPaths).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "清理失效目录" }));
  fireEvent.click(screen.getByRole("button", { name: "清理" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "清理" })).toHaveProperty("disabled", false));
  expect(screen.getByRole("dialog", { name: "清理失效目录？" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "清理" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(api.cleanMissingProjectPaths).toHaveBeenCalledWith("p1");
  expect(screen.getByRole("heading", { name: "工作目录（1）" })).toBeTruthy();
  expect(screen.getByRole("button", { name: /^Session 1/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "清理失效目录" })).toBeNull();
});

it("keeps missing directories referenced by archived sessions out of the cleanup action", async () => {
  const detail = mockDetail();
  detail.workspace_paths[1].exists = false;
  detail.sessions[0].archived_at = "2026-11-08T00:00:00Z";
  vi.mocked(api.getProjectDetail).mockResolvedValue(detail);
  render(<ProjectDetail projectId="p1" navigate={navigate} />);
  await screen.findByRole("heading", { name: "工作目录（2）" });
  expect(screen.getByRole("button", { name: /main.*worktree/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "清理失效目录" })).toBeNull();
});
