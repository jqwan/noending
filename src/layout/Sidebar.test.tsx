// Projects Experience v0.2 §27 — 前端契约：
// Sidebar 只有一等 Projects 导航，不再渲染单个 Project 实体。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
import { api } from "../api";
import type { Route } from "../app/routes";

vi.mock("../api", () => ({
  api: {
    listWorkstreamCards: vi.fn(),
    listProjects: vi.fn(),
  },
}));

beforeEach(() => {
  vi.mocked(api.listWorkstreamCards).mockReset().mockResolvedValue([]);
  vi.mocked(api.listProjects).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

function renderSidebar(route: Route, navigate = vi.fn()) {
  render(<Sidebar route={route} navigate={navigate} onSearch={() => {}} />);
  return navigate;
}

describe("Sidebar projects navigation", () => {
  it("sidebar_has_projects_navigation", async () => {
    const navigate = renderSidebar({ view: "workstreams" });

    const item = await screen.findByText("项目");
    fireEvent.click(item);

    await waitFor(() => {
      expect(navigate).toHaveBeenCalledWith({ view: "projects" });
    });
  });

  it("sidebar_projects_stays_weak_active_on_project_detail", async () => {
    renderSidebar({ view: "project", projectId: "p-1" });
    const item = await screen.findByText("项目");
    // §22 — Project Detail 时保持弱高亮，与 Workstream Detail 的模式一致。
    expect(item.className).toContain("weak");
  });

  it("sidebar_does_not_render_individual_projects", async () => {
    renderSidebar({ view: "workstreams" });
    await screen.findByText("项目"); // 导航项在

    expect(api.listProjects).not.toHaveBeenCalled();
  });
});
