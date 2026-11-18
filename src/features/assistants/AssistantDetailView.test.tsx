import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api";
import { notifyDataChanged } from "../../app/routes";
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
    vi.mocked(api.listProjects).mockResolvedValue([]);
    vi.mocked(api.listWorkstreams).mockResolvedValue([]);
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

  it("preserves instructions draft and handles remote revision conflict via refresh signal", async () => {
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

    // 触发全局刷新信号（如后台同步、外部修改完成）
    await act(async () => {
      notifyDataChanged([]);
    });

    // 验证第二次读取发生，冲突警告浮现，且用户草稿完好保留
    await waitFor(() => {
      expect(api.getAssistantProfile).toHaveBeenCalledTimes(2);
      expect(screen.getByText(/后台档案已更新为版本 r2/)).toBeTruthy();
      expect(textarea).toHaveProperty("value", "用户未保存的本地草稿");
    });

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

  it("handles conflict resolution by discarding draft or force overwriting", async () => {
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

    const textarea = screen.getByDisplayValue("严格遵循系统边界");
    fireEvent.change(textarea, { target: { value: "本地编辑中" } });

    // 模拟后台更新并触发刷新信号
    const remoteNew: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        instructions: "远端最新版本指令",
        revision: 5,
      },
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(remoteNew);

    await act(async () => {
      notifyDataChanged([]);
    });

    await waitFor(() => {
      expect(screen.getByText(/当前草稿基于版本 r1/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "放弃草稿" })).toBeTruthy();
      expect(screen.getByRole("button", { name: "覆盖远端修改" })).toBeTruthy();
    });

    // 1. 覆盖远端修改
    vi.mocked(api.updateAssistantProfile).mockResolvedValue({
      ...remoteNew,
      profile: {
        ...remoteNew.profile,
        instructions: "本地编辑中",
        revision: 6,
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "覆盖远端修改" }));

    await waitFor(() => {
      expect(api.updateAssistantProfile).toHaveBeenCalledWith(
        "asst-1",
        5, // 强制覆盖使用当前远端 revision 5
        { instructions: "本地编辑中" },
      );
      expect(screen.queryByText(/当前草稿基于版本/)).toBeNull();
    });

    // 2. 再次修改后选择放弃草稿
    fireEvent.change(textarea, { target: { value: "又一次本地编辑" } });
    const remoteNewer: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        instructions: "远端最新更新",
        revision: 7,
      },
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(remoteNewer);

    await act(async () => {
      notifyDataChanged([]);
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "放弃草稿" })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: "放弃草稿" }));

    await waitFor(() => {
      expect(screen.getByDisplayValue("远端最新更新")).toBeTruthy();
      expect(screen.queryByText(/当前草稿基于版本/)).toBeNull();
    });
  });

  it("manages responsibilities with loading, error retry, and archived item preservation", async () => {
    const detailWithResp: AssistantDetail = {
      ...mockDetail,
      projects: [
        {
          id: "proj-archived",
          name: "旧归档项目",
          git_id: null,
          name_customized: false,
          visibility: "archived",
          created_at: "2026-11-01T00:00:00Z",
          updated_at: "2026-11-01T00:00:00Z",
        },
      ],
      tasks: [],
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(detailWithResp);

    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText("旧归档项目")).toBeTruthy();
    });

    // 首次点击调整分配，模拟项目读取失败
    vi.mocked(api.listProjects).mockRejectedValueOnce(new Error("网络超时"));
    fireEvent.click(screen.getByRole("button", { name: "调整分配" }));

    await waitFor(() => {
      expect(screen.getByText(/读取项目\/任务失败/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "重试" })).toBeTruthy();
      expect((screen.getByRole("button", { name: "确认分配" }) as HTMLButtonElement).disabled).toBe(true);
    });

    // 点击重试，返回已有归档项目、未分配归档项目、正常项目
    vi.mocked(api.listProjects).mockResolvedValue([
      {
        id: "proj-archived",
        name: "旧归档项目",
        git_id: null,
        name_customized: false,
        visibility: "archived",
        created_at: "2026-11-01T00:00:00Z",
        updated_at: "2026-11-01T00:00:00Z",
      },
      {
        id: "proj-other-archived",
        name: "其他归档项目",
        git_id: null,
        name_customized: false,
        visibility: "archived",
        created_at: "2026-11-01T00:00:00Z",
        updated_at: "2026-11-01T00:00:00Z",
      },
      {
        id: "proj-active",
        name: "活跃项目",
        git_id: null,
        name_customized: false,
        visibility: "normal",
        created_at: "2026-11-10T00:00:00Z",
        updated_at: "2026-11-10T00:00:00Z",
      },
    ]);
    vi.mocked(api.listWorkstreams).mockResolvedValue([]);

    const retryBtn = screen.getByRole("button", { name: "重试" });
    fireEvent.click(retryBtn);

    await waitFor(() => {
      expect(screen.getByText("活跃项目")).toBeTruthy();
      // 已分配的归档项目保留展示且带标记
      expect(screen.getAllByText("旧归档项目").length).toBeGreaterThanOrEqual(1);
      // 未分配的归档项目不可新增（未在列表中展示）
      expect(screen.queryByText("其他归档项目")).toBeNull();
    });

    // 勾选新项目并确认保存
    fireEvent.click(screen.getByRole("checkbox", { name: "活跃项目" }));
    vi.mocked(api.setAssistantResponsibilities).mockResolvedValue({
      ...detailWithResp,
      projects: [
        detailWithResp.projects[0],
        {
          id: "proj-active",
          name: "活跃项目",
          git_id: null,
          name_customized: false,
          visibility: "normal",
          created_at: "2026-11-10T00:00:00Z",
          updated_at: "2026-11-10T00:00:00Z",
        },
      ],
    });

    fireEvent.click(screen.getByRole("button", { name: "确认分配" }));

    await waitFor(() => {
      expect(api.setAssistantResponsibilities).toHaveBeenCalledWith(
        "asst-1",
        1,
        ["proj-archived", "proj-active"],
        [],
      );
    });
  });

  it("supports tri-state runtime override selection in edit profile modal", async () => {
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

    expect(screen.getByLabelText("继承应用默认")).toBeTruthy();
    expect(screen.getByLabelText("Agent CLI 默认")).toBeTruthy();
    expect(screen.getByLabelText("自定义模型与推理")).toBeTruthy();

    // 1. 选择 Agent CLI 默认
    fireEvent.click(screen.getByLabelText("Agent CLI 默认"));

    vi.mocked(api.updateAssistantProfile).mockResolvedValue({
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        runtime_override: { model: null, provider: null, effort: null },
        revision: 2,
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(api.updateAssistantProfile).toHaveBeenCalledWith(
        "asst-1",
        1,
        expect.objectContaining({
          runtime_override: { model: null, provider: null, effort: null },
        }),
      );
    });
  });

  it("handles agent status detection error with retry in edit profile modal", async () => {
    vi.mocked(api.getAgentStatus).mockRejectedValueOnce(new Error("探测超时"));

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

    await waitFor(() => {
      expect(screen.getByText(/读取运行工具状态失败/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "重试" })).toBeTruthy();
    });

    vi.mocked(api.getAgentStatus).mockResolvedValueOnce({
      codex: { terminal_cli: true, detected: true, launch_enabled: true },
      claude_code: { terminal_cli: true, detected: true, launch_enabled: true },
    } as any);

    fireEvent.click(screen.getByRole("button", { name: "重试" }));

    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalled();
    });
  });

  it("uses fixed base revision when saving in edit profile modal even if background refreshed", async () => {
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

    // 用户在弹窗中修改名称
    const nameInput = screen.getByDisplayValue("测试架构师");
    fireEvent.change(nameInput, { target: { value: "新名称" } });

    // 此时后台刷新，更新 revision 到 5
    const remoteUpdated: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        revision: 5,
      },
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(remoteUpdated);

    await act(async () => {
      notifyDataChanged([]);
    });

    // 点击保存：提交必须使用固定基线 revision 1，而非被后台污染的 revision 5
    vi.mocked(api.updateAssistantProfile).mockResolvedValue({
      ...mockDetail,
      profile: { ...mockDetail.profile, name: "新名称", revision: 2 },
    });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(api.updateAssistantProfile).toHaveBeenCalledWith(
        "asst-1",
        1, // 必须是基线 1！
        expect.objectContaining({ name: "新名称" }),
      );
    });
  });

  it("uses fixed base revision when saving responsibilities even if background refreshed", async () => {
    vi.mocked(api.listProjects).mockResolvedValue([mockDetail.project]);
    render(
      <AssistantDetailView
        assistantId="asst-1"
        navigate={navigate}
        goBack={goBack}
      />,
    );

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "调整分配" })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: "调整分配" }));

    const projectChoice = await screen.findByRole("checkbox", { name: mockDetail.project.name });
    fireEvent.click(projectChoice);

    // 此时后台刷新，更新 revision 到 8
    const remoteUpdated: AssistantDetail = {
      ...mockDetail,
      profile: {
        ...mockDetail.profile,
        revision: 8,
      },
    };
    vi.mocked(api.getAssistantProfile).mockResolvedValue(remoteUpdated);

    await act(async () => {
      notifyDataChanged([]);
    });

    // 保存分配：必须使用固定开窗 revision 1
    vi.mocked(api.setAssistantResponsibilities).mockRejectedValue(new Error("版本冲突，请重新读取"));

    await waitFor(() => {
      expect((screen.getByRole("button", { name: "确认分配" }) as HTMLButtonElement).disabled).toBe(false);
    });

    fireEvent.click(screen.getByRole("button", { name: "确认分配" }));

    await waitFor(() => {
      expect(api.setAssistantResponsibilities).toHaveBeenCalledWith(
        "asst-1",
        1, // 必须是固定开窗 revision 1
        [mockDetail.project.id],
        [],
      );
      const dialog = screen.getByRole("dialog", { name: "调整负责项目与任务" });
      expect(within(dialog).getByText(/版本冲突，请重新读取/)).toBeTruthy();
      expect(projectChoice).toHaveProperty("checked", true);
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
