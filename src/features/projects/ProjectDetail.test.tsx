import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectDetail, { clearProjectDetailCache } from "./ProjectDetail";
import { api } from "../../api";
import type { ProjectDetailData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listPreferences: vi.fn().mockResolvedValue([]),
    savePreference: vi.fn(),
    deletePreference: vi.fn(),
    getProjectDetail: vi.fn(),
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
        forked_from_session_id: null,
        started_at: "2026-10-01T00:00:00Z",
        last_activity_at: "2026-10-01T01:00:00Z",
        last_conversation_at: null,
        archived_at: null,
        source_kind: "codex_rollout",
        source_path: "/tmp/1",
        metadata: {},
        source_file_identity: "id-1",
        source_generation: 1,
        source_byte_offset: 0,
        source_last_seen_size: 0,
        source_mtime: null,
        source_prefix_hash: "",
        source_tail_hash: "",
        fact_generation: 1,
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

  it("orders git main worktree first in workspace paths list", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const { container } = render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    const pathRows = container.querySelectorAll(".project-workspace-paths .list-row");
    expect(pathRows.length).toBe(2);
    // Even though /a/linked/worktree is alphabetically earlier, /z/main/worktree is git_kind: "main" and comes first
    expect(pathRows[0].textContent).toContain("/z/main/worktree");
    expect(pathRows[1].textContent).toContain("/a/linked/worktree");
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
