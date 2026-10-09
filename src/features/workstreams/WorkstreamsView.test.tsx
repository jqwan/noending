import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WorkstreamsView from "./WorkstreamsView";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type { WorkstreamCardData } from "../../types";

const state = vi.hoisted(() => ({ cards: [] as WorkstreamCardData[], refresh: vi.fn() }));
vi.mock("./useWorkstreamCards", () => ({ useWorkstreamCards: () => ({ cards: state.cards, defaultAgent: "codex", refresh: state.refresh, loadError: "" }) }));
vi.mock("../../api", () => ({ api: { archiveWorkstream: vi.fn(), restoreWorkstream: vi.fn(), deleteWorkstreamPermanently: vi.fn() } }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const card = (id: string, visibility: WorkstreamCardData["visibility"] = "normal"): WorkstreamCardData => ({ id, projects: [], title: id, description: "", visibility, session_count: 1, path_count: 0, created_at: "", updated_at: "", current_state: null, goal: null, last_activity_at: null, latest_session: null });
beforeEach(() => { vi.clearAllMocks(); viewState.clear(); state.cards = [card("普通任务"), card("归档甲", "archived"), card("归档乙", "archived")]; });
afterEach(cleanup);

it("uses the same searchable cards for both scopes with the correct toolbar actions", () => {
  const navigate = vi.fn();
  const { rerender } = render(<WorkstreamsView navigate={navigate} actionSeq={0} />);
  screen.getByRole("button", { name: "新建任务" });
  expect(screen.queryByRole("button", { name: "删除全部" })).toBeNull();
  screen.getByRole("button", { name: "普通任务" });
  rerender(<WorkstreamsView navigate={navigate} scope="archived" actionSeq={0} />);
  screen.getByRole("button", { name: "删除全部" });
  expect(screen.queryByRole("button", { name: "新建任务" })).toBeNull();
  expect(document.querySelectorAll(".task-card")).toHaveLength(2);
  fireEvent.change(screen.getByRole("textbox", { name: "搜索任务" }), { target: { value: "甲" } });
  expect(document.querySelectorAll(".task-card")).toHaveLength(1);
  screen.getByRole("button", { name: "永久删除归档甲" });
});

it("delete all covers archived tasks even when the board is filtered", async () => {
  vi.mocked(api.deleteWorkstreamPermanently).mockResolvedValue(undefined);
  render(<WorkstreamsView navigate={() => {}} scope="archived" actionSeq={0} />);
  fireEvent.change(screen.getByRole("textbox", { name: "搜索任务" }), { target: { value: "甲" } });
  fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
  const dialog = screen.getByRole("dialog");
  expect(dialog.textContent).toContain("2 个任务");
  fireEvent.click(within(dialog).getByRole("button", { name: "删除全部" }));
  await waitFor(() => expect(api.deleteWorkstreamPermanently).toHaveBeenCalledTimes(2));
  expect(api.deleteWorkstreamPermanently).toHaveBeenCalledWith("归档甲");
  expect(api.deleteWorkstreamPermanently).toHaveBeenCalledWith("归档乙");
  expect(api.deleteWorkstreamPermanently).not.toHaveBeenCalledWith("普通任务");
});

it("keeps card and list views available in both archive scopes", async () => {
  const navigate = vi.fn();
  const { container, rerender } = render(<WorkstreamsView navigate={navigate} actionSeq={0} />);
  expect(container.querySelector(".task-card")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
  expect(container.querySelector(".task-list-row")).toBeTruthy();
  expect(container.querySelector(".task-card")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "归档普通任务" }));
  await waitFor(() => expect(api.archiveWorkstream).toHaveBeenCalledWith("普通任务"));
  rerender(<WorkstreamsView navigate={navigate} scope="archived" actionSeq={0} />);
  expect(container.querySelectorAll(".task-list-row")).toHaveLength(2);
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  screen.getByRole("button", { name: "取消归档归档甲" });
  screen.getByRole("button", { name: "永久删除归档甲" });
  fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
  expect(container.querySelectorAll(".task-card")).toHaveLength(2);
});

it("shows and filters every related project in both board views", () => {
  const multiple = { ...card("跨项目任务"), projects: [{ id: "p-a", name: "项目甲" }, { id: "p-b", name: "项目乙" }] };
  state.cards = [multiple, card("无项目任务")];
  render(<WorkstreamsView navigate={() => {}} actionSeq={0} />);
  const projectFilter = screen.getByText("全部项目").closest("select")!;
  fireEvent.change(projectFilter, { target: { value: "p-b" } });
  expect(document.querySelectorAll(".task-card")).toHaveLength(1);
  screen.getByRole("button", { name: "跨项目任务" });
  expect(screen.queryByRole("button", { name: "无项目任务" })).toBeNull();
  const task = document.querySelector(".task-card")!;
  expect(task.textContent).toContain("项目甲");
  expect(task.textContent).toContain("项目乙");
  fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
  const row = document.querySelector(".task-list-row")!;
  expect(row.textContent).toContain("项目甲");
  expect(row.textContent).toContain("项目乙");
  fireEvent.change(projectFilter, { target: { value: "none" } });
  screen.getByText("无项目任务");
  expect(screen.queryByText("跨项目任务")).toBeNull();
});
