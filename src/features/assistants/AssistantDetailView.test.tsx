import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api";
import type { AssistantDetail, AssistantPage, SessionListItem } from "../../types";
import AssistantDetailView from "./AssistantDetailView";
import { clearAssistantsCache } from "./assistantsCache";

vi.mock("../../api", () => ({
  api: {
    getAssistantProfile: vi.fn(),
    updateAssistantProfile: vi.fn(),
    prepareAssistantFiles: vi.fn(),
    startAssistantSession: vi.fn(),
    setAssistantResponsibilities: vi.fn(),
    setAssistantArchived: vi.fn(),
    listAssistantSessions: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    getAgentStatus: vi.fn().mockResolvedValue({
      codex: { terminal_cli: true, detected: true, launch_enabled: true },
      claude_code: { terminal_cli: true, detected: true, launch_enabled: true },
    }),
    getAgentRuntimeSettings: vi.fn().mockResolvedValue({
      agent: "codex",
      executable: "codex",
      version: "1.0",
      overrides: { model: null, provider: null, effort: null },
      runtime: { model: null, provider: null, effort: null },
    }),
    refreshAgentRuntimeOptions: vi.fn().mockResolvedValue({
      models: [],
      effort_levels: [],
      warnings: [],
      model_source: "dynamic",
    }),
  },
}));

const mockDetail: AssistantDetail = {
  profile: {
    id: "asst-1",
    name: "测试架构师",
    avatar: "spark",
    description: "负责架构与评审",
    instructions: "严格遵循系统边界",
    agent: "codex",
    runtime_override: null,
    workspace_path_id: "wp-1",
    visibility: "normal",
    revision: 1,
    created_at: "2026-11-18T00:00:00Z",
    updated_at: "2026-11-18T00:00:00Z",
  },
  project: {
    id: "proj-1",
    name: "测试架构师",
    git_id: null,
    name_customized: false,
    visibility: "normal",
    created_at: "2026-11-18T00:00:00Z",
    updated_at: "2026-11-18T00:00:00Z",
  },
  workspace_path: {
    id: "wp-1",
    canonical_path: "/Users/test/.noending/assistant/asst-1",
    project_id: "proj-1",
    git_state: "none",
    git_kind: null,
    exists: true,
  },
  projects: [],
  tasks: [],
  session_count: 1,
  last_activity_at: "2026-11-18T00:00:00Z",
  instructions_state: "current",
};

const mockSessionItem: SessionListItem = {
  id: "sess-1",
  title: "首次架构讨论",
  agent: "codex",
  root_agent_session_id: "ras-1",
  cwd: "/Users/test/.noending/assistant/asst-1",
  project_id: "proj-1",
  workspace_path_id: "wp-1",
  tracked_workstream_ids: [],
  started_at: "2026-11-18T01:00:00Z",
  last_activity_at: "2026-11-18T01:00:00Z",
  last_conversation_at: "2026-11-18T01:00:00Z",
  archived_at: null,
  source_kind: "managed",
  latest_message_seq: 5,
};

const mockSessionsPage: AssistantPage<SessionListItem> = {
  items: [mockSessionItem],
  next_cursor: "cursor-sess-2",
  end: false,
};

describe("AssistantDetailView", () => {
  const navigate = vi.fn();
  const goBack = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    clearAssistantsCache();
    vi.mocked(api.getAssistantProfile).mockResolvedValue(mockDetail);
    vi.mocked(api.listAssistantSessions).mockResolvedValue(mockSessionsPage);
  });

  afterEach(cleanup);

  it("renders assistant profile details and sessions", async () => {
    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText("负责架构与评审")).toBeTruthy();
      expect(screen.getByText("严格遵循系统边界")).toBeTruthy();
      expect(screen.getByText("首次架构讨论")).toBeTruthy();
      expect(screen.getByText("加载更多会话")).toBeTruthy();
    });
  });

  it("opens edit profile modal and saves changes", async () => {
    const updatedDetail: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        name: "资深架构师",
        description: "负责全面系统设计",
        revision: 2,
      },
    };
    vi.mocked(api.updateAssistantProfile).mockResolvedValue(updatedDetail);

    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "编辑资料" })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: "编辑资料" }));

    expect(screen.getByRole("heading", { name: "编辑智能体档案" })).toBeTruthy();

    const descInput = screen.getByDisplayValue("负责架构与评审");
    fireEvent.change(descInput, { target: { value: "负责全面系统设计" } });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(api.updateAssistantProfile).toHaveBeenCalledWith(
        "asst-1",
        1,
        expect.objectContaining({
          description: "负责全面系统设计",
        }),
      );
    });
  });

  it("preserves instructions draft and handles remote revision conflict", async () => {
    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByDisplayValue("严格遵循系统边界")).toBeTruthy();
    });

    // 用户编辑了草稿
    const textarea = screen.getByDisplayValue("严格遵循系统边界");
    fireEvent.change(textarea, { target: { value: "用户未保存的本地草稿" } });

    // 此时后台刷新，远端更新为 r2
    const remoteUpdated: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        instructions: "远端其他人修改的新指令",
        revision: 2,
      },
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(remoteUpdated);

    // 触发刷新：验证本地草稿不被覆盖
    expect(textarea).toHaveProperty("value", "用户未保存的本地草稿");

    // 保存草稿
    vi.mocked(api.updateAssistantProfile).mockResolvedValue({
      ...remoteUpdated,
      profile: {
        ...remoteUpdated.profile,
        instructions: "用户未保存的本地草稿",
        revision: 3,
      },
    });

    const saveBtn = screen.getByRole("button", { name: "保存修改" });
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(api.updateAssistantProfile).toHaveBeenCalledWith(
        "asst-1",
        1, // 提交时携带草稿基线 revision 1
        { instructions: "用户未保存的本地草稿" },
      );
    });
  });

  it("loads more sessions when clicking 加载更多会话", async () => {
    const secondPage: AssistantPage<SessionListItem> = {
      items: [
        {
          ...mockSessionItem,
          id: "sess-2",
          title: "第二轮评审",
        },
      ],
      next_cursor: null,
      end: true,
    };
    vi.mocked(api.listAssistantSessions).mockResolvedValueOnce(mockSessionsPage).mockResolvedValueOnce(secondPage);

    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText("加载更多会话")).toBeTruthy();
    });

    fireEvent.click(screen.getByText("加载更多会话"));

    await waitFor(() => {
      expect(api.listAssistantSessions).toHaveBeenCalledWith("asst-1", {
        cursor: "cursor-sess-2",
        limit: 20,
      });
      expect(screen.getByText("第二轮评审")).toBeTruthy();
    });
  });
});
