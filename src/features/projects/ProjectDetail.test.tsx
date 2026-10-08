import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectDetail, { clearProjectDetailCache } from "./ProjectDetail";
import { api } from "../../api";
import type { ProjectDetailData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getProjectDetail: vi.fn(),
    renameProject: vi.fn(),
    refreshProjectWorkspace: vi.fn(),
    getAgentStatus: vi.fn(),
    continueSessionDesktop: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const navigate = vi.fn();

beforeEach(() => {
  clearProjectDetailCache();
  navigate.mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
  vi.mocked(api.getAgentStatus).mockResolvedValue({
    codex: {
      name: "codex",
      detected: true,
      executable: "/bin/codex",
      version: "1.0.0",
      terminal_cli: true,
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
    project: {
      id: "p1",
      name: "Test Project",
      description: "",
      git_id: "git-1",
      name_customized: false,
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
        owner_workstream_id: null,
        forked_from_session_id: null,
        started_at: "2026-10-01T00:00:00Z",
        last_activity_at: "2026-10-01T01:00:00Z",
        last_conversation_at: null,
        trashed_at: null,
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
  it("renders + button in sessions section and navigates to new-session on click", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    const newSessionBtn = screen.getByRole("button", { name: "新建会话" });
    expect(newSessionBtn).toBeTruthy();

    fireEvent.click(newSessionBtn);
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p1" });
  });

  it("orders git main worktree first in workspace paths list", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    const { container } = render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    const pathRows = container.querySelectorAll("aside .list-row");
    expect(pathRows.length).toBe(2);
    // Even though /a/linked/worktree is alphabetically earlier, /z/main/worktree is git_kind: "main" and comes first
    expect(pathRows[0].textContent).toContain("/z/main/worktree");
    expect(pathRows[1].textContent).toContain("/a/linked/worktree");
  });

  it("renders continue session button on session rows and stops row navigation", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Session 1");
    const continueBtn = screen.getByRole("button", { name: /在桌面应用中继续Session 1/ });
    expect(continueBtn).toBeTruthy();

    fireEvent.click(continueBtn);
    expect(api.continueSessionDesktop).toHaveBeenCalledWith("s1");
    // Row click should NOT have navigated to session detail
    expect(navigate).not.toHaveBeenCalled();
  });

  it("does not render 询问助手 button in header", async () => {
    vi.mocked(api.getProjectDetail).mockResolvedValue(mockDetail());
    render(<ProjectDetail projectId="p1" navigate={navigate} />);

    await screen.findByText("Test Project");
    expect(screen.queryByRole("button", { name: "询问助手" })).toBeNull();
  });
});
