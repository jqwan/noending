import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import NavigationRail from "./NavigationRail";
import type { Route } from "../app/routes";

afterEach(cleanup);

it.each([
  ["会话", "sessions"], ["任务", "workstreams"], ["项目", "projects"],
  ["代理", "agents"], ["助手", "assistant"], ["设置", "settings"],
] as const)("opens the existing %s page", (label, view) => {
  const navigate = vi.fn();
  render(<NavigationRail route={{ view: "new-session" }} navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: label }));
  expect(navigate).toHaveBeenCalledWith({ view });
});

it.each([
  [{ view: "terminal", terminalId: "t1" }, "会话"],
  [{ view: "session", sessionId: "s1" }, "会话"],
  [{ view: "workstream", workstreamId: "w1" }, "任务"],
  [{ view: "project", projectId: "p1" }, "项目"],
] as [Route, string][])("highlights the owning section for %j", (route, label) => {
  render(<NavigationRail route={route} navigate={vi.fn()} />);
  expect(screen.getByRole("button", { name: label }).getAttribute("aria-current")).toBe("page");
});
