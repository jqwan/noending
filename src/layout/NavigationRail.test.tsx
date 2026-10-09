import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import NavigationRail from "./NavigationRail";
import type { Route } from "../app/routes";

afterEach(cleanup);

it.each([
  ["任务", "workstreams"], ["项目", "projects"],
  ["代理", "agents"], ["助手", "assistant"], ["设置", "settings"],
] as const)("opens the existing %s page", (label, view) => {
  const navigate = vi.fn();
  render(<NavigationRail route={{ view: "new-session" }} navigate={navigate} onSessions={vi.fn()} onSearch={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: label }));
  expect(navigate).toHaveBeenCalledWith({ view });
});

it.each([
  [{ view: "terminal", terminalId: "t1" }, "首页"],
  [{ view: "session", sessionId: "s1" }, "首页"],
  [{ view: "search", query: "test" }, "搜索"],
  [{ view: "workstream", workstreamId: "w1" }, "任务"],
  [{ view: "project", projectId: "p1" }, "项目"],
] as [Route, string][])("highlights the owning section for %j", (route, label) => {
  render(<NavigationRail route={route} navigate={vi.fn()} onSessions={vi.fn()} onSearch={vi.fn()} />);
  expect(screen.getByRole("button", { name: label }).getAttribute("aria-current")).toBe("page");
});

it("opens search and restores the session section through their dedicated callbacks", () => {
  const navigate = vi.fn();
  const onSearch = vi.fn();
  const onSessions = vi.fn();
  render(<NavigationRail route={{ view: "projects" }} navigate={navigate} onSessions={onSessions} onSearch={onSearch} />);
  fireEvent.click(screen.getByRole("button", { name: "搜索" }));
  expect(onSearch).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "首页" }));
  expect(onSessions).toHaveBeenCalledOnce();
  expect(navigate).not.toHaveBeenCalled();
});
