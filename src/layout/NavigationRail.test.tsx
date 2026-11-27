import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import NavigationRail from "./NavigationRail";
import type { Route } from "../app/routes";

afterEach(cleanup);

it("opens the existing primary pages", () => {
  const sections = [
    ["会话", "sessions"], ["任务", "workstreams"], ["项目", "projects"],
    ["智能体", "projects"], ["运行工具", "agents"], ["技能", "skills"], ["设置", "settings"],
  ] as const;

  for (const [label, view] of sections) {
    const navigate = vi.fn();
    const { unmount } = render(<NavigationRail route={{ view: "new-session" }} navigate={navigate} />);
    fireEvent.click(screen.getByRole("button", { name: label }));
    expect(navigate).toHaveBeenCalledWith(label === "智能体" ? { view, assistantOnly: true } : { view });
    unmount();
  }
});

it("highlights the section that owns the route and returns to its board when clicked", () => {
  const cases: [Route, string, Route][] = [
    [{ view: "terminal", terminalId: "t1" }, "会话", { view: "sessions" }],
    [{ view: "session", sessionId: "s1" }, "会话", { view: "sessions" }],
    [{ view: "workstream", workstreamId: "w1" }, "任务", { view: "workstreams" }],
    [{ view: "project", projectId: "p1" }, "项目", { view: "projects" }],
    [{ view: "project", projectId: "p1", fromAssistants: true }, "智能体", { view: "projects", assistantOnly: true }],
    [{ view: "skill", resourceId: "skill1" }, "技能", { view: "skills" }],
    [{ view: "settings" }, "设置", { view: "settings" }],
  ];
  const navigate = vi.fn();
  const { rerender } = render(<NavigationRail route={cases[0][0]} navigate={navigate} />);
  for (const [route, label, board] of cases) {
    rerender(<NavigationRail route={route} navigate={navigate} />);
    const section = screen.getByRole("button", { name: label });
    expect(section.getAttribute("aria-current")).toBe("page");
    fireEvent.click(section);
    expect(navigate).toHaveBeenLastCalledWith(board);
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
