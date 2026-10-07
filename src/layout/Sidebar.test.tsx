// Sidebar 契约：一等导航 + 「运行中」终端列表（registry 运行时事实，事件刷新）。
// 旧的最近任务/固定列表已由运行终端取代。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
import { api } from "../api";
import { EVT_TERMINALS } from "../app/routes";
import type { Route } from "../app/routes";
import type { TerminalSummary } from "../types";

vi.mock("../api", () => ({
  api: {
    terminalList: vi.fn(),
  },
}));

beforeEach(() => {
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
});

afterEach(cleanup);

function renderSidebar(route: Route = { view: "home" }, navigate = vi.fn()) {
  render(<Sidebar route={route} navigate={navigate} onSearch={() => {}} />);
  return navigate;
}

function terminal(over: Partial<TerminalSummary>): TerminalSummary {
  return {
    terminal_id: "t-1",
    session_id: null,
    agent: "codex",
    cwd: "/repo/x",
    created_at: new Date().toISOString(),
    live: true,
    exit_code: null,
    session_title: null,
    ...over,
  };
}

describe("Sidebar navigation", () => {
  it("provides an explicit home entry", () => {
    const navigate = renderSidebar();
    const home = screen.getByRole("button", { name: "首页" });
    expect(home.className).toContain("active");
    fireEvent.click(home);
    expect(navigate).toHaveBeenCalledWith({ view: "home" });
  });

  it("sidebar_has_projects_navigation", async () => {
    const navigate = renderSidebar({ view: "workstreams" });

    const item = await screen.findByText("项目");
    fireEvent.click(item);

    await waitFor(() => {
      expect(navigate).toHaveBeenCalledWith({ view: "projects" });
    });
  });

  it("sidebar_has_agents_navigation", async () => {
    const navigate = renderSidebar({ view: "workstreams" });

    const item = await screen.findByText("代理");
    fireEvent.click(item);

    await waitFor(() => {
      expect(navigate).toHaveBeenCalledWith({ view: "agents" });
    });
  });

  it("sidebar_agents_is_active_when_route_is_agents", async () => {
    renderSidebar({ view: "agents" });
    const item = await screen.findByText("代理");
    expect(item.className).toContain("active");
  });

  it("sidebar_settings_is_active_when_route_is_settings", () => {
    const navigate = renderSidebar({ view: "settings" });
    const settings = screen.getByRole("button", { name: /设置/ });
    expect(settings.className).toContain("active");
    fireEvent.click(settings);
    expect(navigate).toHaveBeenCalledWith({ view: "settings" });
  });
});

describe("Sidebar 运行中终端", () => {
  it("hides the section when no terminal is running", async () => {
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalled());
    expect(screen.queryByText("运行中")).toBeNull();
  });

  it("shows live terminals; unbound reads 新终端, bound reads the session name", async () => {
    vi.mocked(api.terminalList).mockResolvedValue([
      terminal({ terminal_id: "t-1" }),
      terminal({ terminal_id: "t-2", session_id: "s1", session_title: "修复布局" }),
    ]);
    const navigate = renderSidebar();

    expect(await screen.findByText("新终端")).toBeTruthy();
    expect(screen.getByText("修复布局")).toBeTruthy();

    fireEvent.click(screen.getByText("新终端"));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-1" });
    fireEvent.click(screen.getByText("修复布局"));
    expect(navigate).toHaveBeenCalledWith({ view: "terminal", terminalId: "t-2" });
  });

  it("refetches once per terminals-changed event — no polling", async () => {
    renderSidebar();
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(1));

    window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
    window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
    await waitFor(() => expect(api.terminalList).toHaveBeenCalledTimes(3));
  });
});
