import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import NavigationRail from "./NavigationRail";
import type { Route } from "../app/routes";

afterEach(cleanup);

it("opens the existing primary pages", () => {
  const sections = [
    ["会话", "sessions"], ["任务", "workstreams"], ["项目", "projects"],
    ["Agent", "agents"], ["扩展", "extensions"], ["设置", "settings"],
  ] as const;

  for (const [label, view] of sections) {
    const navigate = vi.fn();
    const { unmount } = render(<NavigationRail route={{ view: "new-session" }} navigate={navigate} />);
    fireEvent.click(screen.getByRole("button", { name: label }));
    expect(navigate).toHaveBeenCalledWith({ view });
    unmount();
  }
});

it("highlights the owning section for routes", () => {
  const cases: [Route, string][] = [
    [{ view: "terminal", terminalId: "t1" }, "会话"],
    [{ view: "session", sessionId: "s1" }, "会话"],
    [{ view: "sessions" }, "会话"],
    [{ view: "workstream", workstreamId: "w1" }, "任务"],
    [{ view: "project", projectId: "p1" }, "项目"],
    [{ view: "extensions", projectId: "p1" }, "扩展"],
    [{ view: "extension", resourceId: "skill1", kind: "skill" }, "扩展"],
  ];

  for (const [route, label] of cases) {
    const { unmount } = render(<NavigationRail route={route} navigate={vi.fn()} />);
    expect(screen.getByRole("button", { name: label }).getAttribute("aria-current")).toBe("page");
    unmount();
  }
});

it("jumps to its root board interface when clicking the active section icon", () => {
  const cases: [Route, string, Route][] = [
    [{ view: "session", sessionId: "s1" }, "会话", { view: "sessions" }],
    [{ view: "workstream", workstreamId: "w1" }, "任务", { view: "workstreams" }],
    [{ view: "project", projectId: "p1" }, "项目", { view: "projects" }],
    [{ view: "extension", resourceId: "skill1", kind: "skill" }, "扩展", { view: "extensions" }],
    [{ view: "settings", section: "appearance" }, "设置", { view: "settings" }],
  ];

  for (const [route, label, expected] of cases) {
    const navigate = vi.fn();
    const { unmount } = render(<NavigationRail route={route} navigate={navigate} />);
    fireEvent.click(screen.getByRole("button", { name: label }));
    expect(navigate).toHaveBeenCalledWith(expected);
    unmount();
  }
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
