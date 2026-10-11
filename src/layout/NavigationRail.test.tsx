import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import NavigationRail from "./NavigationRail";
import type { Route } from "../app/routes";

afterEach(cleanup);

it.each([
  ["会话", "sessions"], ["任务", "workstreams"], ["项目", "projects"],
  ["代理", "agents"], ["设置", "settings"],
] as const)("opens the existing %s page", (label, view) => {
  const navigate = vi.fn();
  render(<NavigationRail route={{ view: "new-session" }} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: label }));
  expect(navigate).toHaveBeenCalledWith({ view });
});

it.each([
  [{ view: "terminal", terminalId: "t1" }, "会话"],
  [{ view: "session", sessionId: "s1" }, "会话"],
  [{ view: "sessions" }, "会话"],
  [{ view: "workstream", workstreamId: "w1" }, "任务"],
  [{ view: "project", projectId: "p1" }, "项目"],
] as [Route, string][])("highlights the owning section for %j", (route, label) => {
  render(<NavigationRail route={route} navigate={vi.fn()} />);
  expect(screen.getByRole("button", { name: label }).getAttribute("aria-current")).toBe("page");
});

it("navigates to sessions board on sessions click", () => {
  const navigate = vi.fn();
  render(<NavigationRail route={{ view: "projects" }} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "sessions" });
});

it.each([
  [{ view: "session", sessionId: "s1" }, "会话", { view: "sessions" }],
  [{ view: "workstream", workstreamId: "w1" }, "任务", { view: "workstreams" }],
  [{ view: "project", projectId: "p1" }, "项目", { view: "projects" }],
  [{ view: "settings", section: "appearance" }, "设置", { view: "settings" }],
] as [Route, string, Route][])("clicking the active section icon %s jumps to its root board interface %j", (route, label, expected) => {
  const navigate = vi.fn();
  render(<NavigationRail route={route} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: label }));
  expect(navigate).toHaveBeenCalledWith(expected);
});

it("restores the last visited sub-interface when clicking other section icons", () => {
  const navigate = vi.fn();
  const sessionRoute: Route = { view: "session", sessionId: "s-100", entry: "conversation" };
  const workstreamRoute: Route = { view: "workstream", workstreamId: "w-200" };
  const projectRoute: Route = { view: "project", projectId: "p-300" };

  const { rerender } = render(<NavigationRail route={sessionRoute} navigate={navigate} />);

  // Switch to workstreams section
  rerender(<NavigationRail route={workstreamRoute} navigate={navigate} />);

  // Clicking "会话" (other section) should restore the session subpage
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(navigate).toHaveBeenLastCalledWith(sessionRoute);

  // Switch to project section
  rerender(<NavigationRail route={projectRoute} navigate={navigate} />);

  // Clicking "任务" (other section) should restore the workstream subpage
  fireEvent.click(screen.getByRole("button", { name: "任务" }));
  expect(navigate).toHaveBeenLastCalledWith(workstreamRoute);

  // Clicking "会话" (other section) should still restore the session subpage
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(navigate).toHaveBeenLastCalledWith(sessionRoute);

  // Switch to session subpage, then clicking "会话" (same section) should jump to sessions board
  rerender(<NavigationRail route={sessionRoute} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(navigate).toHaveBeenLastCalledWith({ view: "sessions" });
});

it("sanitizes modal actions when remembering last route", () => {
  const navigate = vi.fn();
  const { rerender } = render(<NavigationRail route={{ view: "workstreams", action: "new" }} navigate={navigate} />);

  // Switch to sessions
  rerender(<NavigationRail route={{ view: "sessions" }} navigate={navigate} />);

  // Click "任务" (other section) -> should not carry action: "new"
  fireEvent.click(screen.getByRole("button", { name: "任务" }));
  expect(navigate).toHaveBeenCalledWith({ view: "workstreams" });
});

it("restores an assistant terminal through the unified session section", () => {
  const navigate = vi.fn();
  const assistant: Route = { view: "terminal", terminalId: "assistant-terminal" };
  const { rerender } = render(<NavigationRail route={assistant} navigate={navigate} />);
  expect(screen.queryByRole("button", { name: "助手" })).toBeNull();
  expect(screen.getByRole("button", { name: "会话" }).getAttribute("aria-current")).toBe("page");
  rerender(<NavigationRail route={{ view: "projects" }} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(navigate).toHaveBeenLastCalledWith(assistant);
});
