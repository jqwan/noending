import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionCards from "./SessionTable";
import type { Session } from "../../types";
import { viewState } from "../../hooks/useViewState";
beforeEach(() => viewState.clear());
afterEach(cleanup);
const session = { id: "s1", title: "修复布局", agent: "codex", cwd: "/test/project", started_at: "2026-09-21T00:00:00Z" } as Session;
it("opens details separately from resume and trash actions", () => {
  const open = vi.fn(); const resume = vi.fn(); const trash = vi.fn();
  render(<SessionCards sessions={[session]} bindings={new Map()} projectNameById={new Map()} onOpen={open} onResume={resume} onTrash={trash} />);
  fireEvent.click(screen.getByText("继续"));
  expect(resume).toHaveBeenCalledWith("s1");
  fireEvent.click(screen.getByLabelText("将修复布局移入回收站"));
  expect(trash).toHaveBeenCalledWith("s1");
  expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("修复布局"));
  expect(open).toHaveBeenCalledWith("s1");
});
it("limits initial rows and reveals more without losing sessions", () => {
  render(<SessionCards sessions={Array.from({ length: 101 }, (_, i) => ({ ...session, id: String(i), title: `会话${i}` }))} bindings={new Map()} projectNameById={new Map()} onOpen={() => {}} onResume={() => {}} onTrash={() => {}} />);
  expect(screen.queryByText("会话100")).toBeNull();
  fireEvent.click(screen.getByText("显示更多（剩余 1）"));
  expect(screen.getByText("会话100")).toBeTruthy();
});
