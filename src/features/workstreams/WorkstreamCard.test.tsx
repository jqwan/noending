import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamCard from "./WorkstreamCard";
import type { WorkstreamCardData } from "../../types";
vi.mock("../sessions/NewSessionModal", () => ({ default: () => <div role="dialog">新建面板</div> }));
vi.mock("../sessions/ResumeSessionModal", () => ({ default: () => <div role="dialog">继续面板</div> }));
afterEach(cleanup);
const card = { id: "w1", title: "优化看板", lifecycle: "active", visibility: "normal", session_count: 1, latest_session: { id: "s1", agent: "codex" } } as WorkstreamCardData;
it("keeps card navigation separate from session actions", () => {
  const navigate = vi.fn();
  render(<WorkstreamCard card={card} mode="full" navigate={navigate} defaultAgent="codex" />);
  fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
  expect(screen.getByText("新建面板")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "继续" }));
  expect(screen.getByText("继续面板")).toBeTruthy();
  expect(navigate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "优化看板" }));
  expect(navigate).toHaveBeenCalledTimes(1);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });
});
it("omits launch actions for archived tasks", () => {
  render(<WorkstreamCard card={{ ...card, visibility: "archived" }} mode="full" navigate={() => {}} defaultAgent="codex" />);
  expect(screen.queryByRole("button", { name: "继续" })).toBeNull();
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  expect(screen.getByText("回收站")).toBeTruthy();
});
