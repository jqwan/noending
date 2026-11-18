import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api";
import { notifyDataChanged } from "../../app/routes";
import type { AssistantCard, AssistantPage } from "../../types";
import { clearAssistantsCache } from "./assistantsCache";
import AssistantsView from "./AssistantsView";

vi.mock("../../api", () => ({
  api: {
    listAssistantProfiles: vi.fn(),
    createAssistantProfile: vi.fn(),
    startAssistantSession: vi.fn(),
    getAgentStatus: vi.fn().mockResolvedValue({
      codex: { terminal_cli: true, detected: true, launch_enabled: true },
      claude_code: { terminal_cli: true, detected: true, launch_enabled: true },
    }),
  },
}));

const mockCard: AssistantCard = {
  id: "asst-1",
  name: "测试架构师",
  avatar: "spark",
  description: "负责梳理需求规格",
  agent: "codex",
  runtime_override: null,
  visibility: "normal",
  revision: 1,
  directory: "/Users/test/.noending/assistant/asst-1",
  project_id: "proj-asst-1",
  directory_exists: true,
  session_count: 3,
  project_count: 1,
  task_count: 2,
  last_activity_at: "2026-11-18T00:00:00Z",
  created_at: "2026-11-17T15:00:00Z",
  updated_at: "2026-11-18T00:00:00Z",
};

const mockPage: AssistantPage<AssistantCard> = {
  items: [mockCard],
  next_cursor: null,
  end: true,
};

describe("AssistantsView", () => {
  const navigate = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    clearAssistantsCache();
    vi.mocked(api.listAssistantProfiles).mockResolvedValue(mockPage);
  });

  afterEach(cleanup);

  it("renders assistant cards list and responds to clicks", async () => {
    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByText("测试架构师")).toBeTruthy();
      expect(screen.getByText("负责梳理需求规格")).toBeTruthy();
      expect(screen.getByText(/3 次会话/)).toBeTruthy();
    });

    fireEvent.click(screen.getByText("测试架构师"));
    expect(navigate).toHaveBeenCalledWith({
      view: "assistant",
      assistantId: "asst-1",
      initialTitle: "测试架构师",
    });
  });

  it("launches session on clicking 开始交流", async () => {
    vi.mocked(api.startAssistantSession).mockResolvedValue({
      terminal_id: "term-123",
      launched_via: "embedded_terminal",
      command_line: "codex",
      note: "started",
    });

    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /开始交流/ })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: /开始交流/ }));

    await waitFor(() => {
      expect(api.startAssistantSession).toHaveBeenCalledWith("asst-1");
      expect(navigate).toHaveBeenCalledWith({
        view: "terminal",
        terminalId: "term-123",
        initialTitle: "测试架构师",
        initialAgent: "codex",
      });
    });
  });

  it("opens create modal and submits new assistant", async () => {
    vi.mocked(api.createAssistantProfile).mockResolvedValue({
      profile: {
        id: "asst-new",
        name: "新建助手",
        avatar: "agent",
        description: "新职责",
        instructions: "新守则",
        agent: "claude_code",
        runtime_override: null,
        workspace_path_id: "wp-new",
        visibility: "normal",
        revision: 1,
        created_at: "2026-11-18T00:00:00Z",
        updated_at: "2026-11-18T00:00:00Z",
      },
      project: { id: "p-new", name: "新建助手", git_id: null, name_customized: false, visibility: "normal", created_at: "", updated_at: "" },
      workspace_path: { id: "wp-new", canonical_path: "/test", project_id: "p-new", git_state: "none", git_kind: null, exists: true },
      projects: [],
      tasks: [],
      session_count: 0,
      last_activity_at: null,
      instructions_state: "current",
    });

    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "新建智能体" })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: "新建智能体" }));

    expect(screen.getByRole("heading", { name: "新建智能体" })).toBeTruthy();

    const nameInput = screen.getByPlaceholderText(/例如：架构师小助手/);
    fireEvent.change(nameInput, { target: { value: "新建助手" } });

    fireEvent.click(screen.getByRole("button", { name: "创建智能体" }));

    await waitFor(() => {
      expect(api.createAssistantProfile).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "新建助手",
        }),
      );
      expect(navigate).toHaveBeenCalledWith({
        view: "assistant",
        assistantId: "asst-new",
        initialTitle: "新建助手",
      });
    });
  });

  it("supports server query filtering and cursor pagination with 加载更多", async () => {
    const page1: AssistantPage<AssistantCard> = {
      items: [mockCard],
      next_cursor: "cursor-page-2",
      end: false,
    };
    const card2: AssistantCard = {
      ...mockCard,
      id: "asst-2",
      name: "测试安全官",
    };
    const page2: AssistantPage<AssistantCard> = {
      items: [card2],
      next_cursor: null,
      end: true,
    };

    vi.mocked(api.listAssistantProfiles).mockResolvedValueOnce(page1).mockResolvedValueOnce(page2);

    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByText("加载更多")).toBeTruthy();
    });

    fireEvent.click(screen.getByText("加载更多"));

    await waitFor(() => {
      expect(api.listAssistantProfiles).toHaveBeenCalledWith(
        expect.objectContaining({
          cursor: "cursor-page-2",
          limit: 50,
        }),
      );
      expect(screen.getByText("测试安全官")).toBeTruthy();
    });
  });

  it("handles agent detection failure with retry in create modal", async () => {
    vi.mocked(api.getAgentStatus).mockRejectedValueOnce(new Error("网络或探测错误"));

    render(<AssistantsView action="new" navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByText(/读取运行工具状态失败/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "重试" })).toBeTruthy();
    });

    vi.mocked(api.getAgentStatus).mockResolvedValueOnce({
      codex: { terminal_cli: true, detected: true, launch_enabled: true },
    } as any);

    fireEvent.click(screen.getByRole("button", { name: "重试" }));

    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalled();
    });
  });

  it("retains existing cards and shows error notice when background refresh fails", async () => {
    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByText("测试架构师")).toBeTruthy();
    });

    // 模拟后台静默刷新失败
    vi.mocked(api.listAssistantProfiles).mockRejectedValueOnce(new Error("网络中断"));

    await act(async () => {
      notifyDataChanged([]);
    });

    await waitFor(() => {
      expect(screen.getByText(/网络中断/)).toBeTruthy();
      // 关键行为：已有卡片保留展示，不被错误态整体隐藏！
      expect(screen.getByText("测试架构师")).toBeTruthy();
      expect(screen.getByRole("button", { name: "重试" })).toBeTruthy();
    });
  });

  it("resets loadingMore on query or refresh to prevent permanent pagination freeze", async () => {
    const page1: AssistantPage<AssistantCard> = {
      items: [mockCard],
      next_cursor: "cursor-1",
      end: false,
    };
    vi.mocked(api.listAssistantProfiles).mockResolvedValue(page1);

    render(<AssistantsView navigate={navigate} />);

    await waitFor(() => {
      expect(screen.getByText("加载更多")).toBeTruthy();
    });

    // 触发刷新：验证加载更多状态可正常点击，不会被锁死
    await act(async () => {
      notifyDataChanged([]);
    });

    const loadMoreBtn = screen.getByRole("button", { name: "加载更多" });
    expect((loadMoreBtn as HTMLButtonElement).disabled).toBe(false);
  });
});
