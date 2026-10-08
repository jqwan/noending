import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WorkstreamsView from "./WorkstreamsView";
import { clearWorkstreamCardsCache } from "./useWorkstreamCards";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type { WorkstreamCardData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listWorkstreamCards: vi.fn(),
    getDefaultAgent: vi.fn(),
    listWorkstreamReviewSummaries: vi.fn(),
  },
}));

const mockTask: WorkstreamCardData = {
  id: "w1",
  title: "测试任务",
  description: "描述内容",
  goal: "目标内容",
  lifecycle: "active",
  visibility: "normal",
  session_count: 2,
  path_count: 1,
  primary_path: "/path/to/project",
  project_id: "p1",
  project_name: "测试项目",
  current_state: "正在运行",
  latest_session: null,
  created_at: "2026-10-01T10:00:00Z",
  updated_at: "2026-10-01T10:00:00Z",
  last_activity_at: "2026-10-01T10:00:00Z",
};

beforeEach(() => {
  clearWorkstreamCardsCache();
  viewState.clear();
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([mockTask]);
  vi.mocked(api.getDefaultAgent).mockReset().mockResolvedValue("codex");
  vi.mocked(api.listWorkstreamReviewSummaries).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

describe("WorkstreamsView", () => {
  it("toggles between card and list view", async () => {
    const { container } = render(<WorkstreamsView navigate={vi.fn()} actionSeq={0} />);

    await screen.findByText("测试任务");
    expect(container.querySelector(".board-grid")).toBeTruthy();
    expect(container.querySelector(".task-list")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(container.querySelector(".task-list")).toBeTruthy();
    expect(container.querySelector(".board-grid")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(container.querySelector(".board-grid")).toBeTruthy();
    expect(container.querySelector(".task-list")).toBeNull();
  });
});
