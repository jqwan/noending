import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import TaskItems from "./TaskItems";
import type { TaskItem } from "../../types";

let stored: TaskItem[];
const first = (): TaskItem => ({ id: "work-1", task_id: "task", title: "实现与验收", description: "保证已有数据完整", details: ["已复现问题"], completed: false, revision: 1, created_at: "2026-10-25T00:00:00Z", updated_at: "2026-10-25T00:00:00Z" });
beforeEach(() => {
  stored = [];
  vi.spyOn(api, "listTaskItems").mockImplementation(async task => structuredClone(stored.filter(item => item.task_id === task)));
  vi.spyOn(api, "createTaskItem").mockImplementation(async (task, content) => {
    const item = { ...first(), task_id: task, title: content.title, description: content.description ?? "", details: [] };
    stored.push(item); return structuredClone(item);
  });
  vi.spyOn(api, "updateTaskItem").mockImplementation(async (task, id, revision, patch) => {
    const index = stored.findIndex(item => item.task_id === task && item.id === id);
    if (index < 0 || stored[index].revision !== revision) throw new Error("事项版本已变化");
    stored[index] = { ...stored[index], ...patch, revision: revision + 1 };
    return structuredClone(stored[index]);
  });
  vi.spyOn(api, "deleteTaskItem").mockImplementation(async (task, id, revision) => {
    const index = stored.findIndex(item => item.task_id === task && item.id === id);
    if (index < 0 || stored[index].revision !== revision) throw new Error("事项版本已变化");
    stored.splice(index, 1);
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it("creates work, preserves progress across completion and reopening, and restores it on remount", async () => {
  const firstRender = render(<TaskItems taskId="task" />);
  await screen.findByText("暂无任务事项");
  fireEvent.click(screen.getByRole("button", { name: "新增事项" }));
  const form = screen.getByRole("form", { name: "新增事项" });
  fireEvent.change(within(form).getByRole("textbox", { name: "事项标题" }), { target: { value: "修复同步" } });
  fireEvent.change(within(form).getByRole("textbox", { name: "事项描述" }), { target: { value: "历史保持完整" } });
  fireEvent.click(within(form).getByRole("button", { name: "保存" }));
  await screen.findByRole("region", { name: "修复同步" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：修复同步" }));
  fireEvent.click(screen.getByRole("button", { name: "添加细则" }));
  fireEvent.change(screen.getByRole("textbox", { name: "细则 1" }), { target: { value: "回归验证通过" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByRole("button", { name: "标记完成：修复同步" });
  fireEvent.click(screen.getByRole("button", { name: "标记完成：修复同步" }));
  await screen.findByRole("button", { name: "标记未完成：修复同步" });
  screen.getByRole("region", { name: "修复同步" });
  screen.getByText("历史保持完整");
  fireEvent.click(screen.getByRole("button", { name: "展开修复同步的细则" }));
  screen.getByText("回归验证通过");
  fireEvent.click(screen.getByRole("button", { name: "标记未完成：修复同步" }));
  await screen.findByRole("button", { name: "标记完成：修复同步" });
  firstRender.unmount();
  render(<TaskItems taskId="task" />);
  await screen.findByRole("region", { name: "修复同步" });
  expect(stored[0]).toMatchObject({ completed: false, description: "历史保持完整", details: ["回归验证通过"] });
});

it("requires a second action to permanently delete, and the deleted work stays gone on remount", async () => {
  stored = [first()];
  const view = render(<TaskItems taskId="task" />);
  await screen.findByRole("region", { name: "实现与验收" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：实现与验收" }));
  fireEvent.click(screen.getByRole("button", { name: "删除事项" }));
  screen.getByRole("form", { name: "编辑事项：实现与验收" });
  fireEvent.click(screen.getByRole("button", { name: "保留事项" }));
  expect(screen.queryByRole("button", { name: "确认删除" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "删除事项" }));
  fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
  await screen.findByText("暂无任务事项");
  view.unmount();
  render(<TaskItems taskId="task" />);
  await screen.findByText("暂无任务事项");
  expect(screen.queryByRole("region", { name: "实现与验收" })).toBeNull();
});

it("refreshes external edits while preserving a conflicting draft and prevents overwriting the newer version", async () => {
  stored = [first()];
  render(<TaskItems taskId="task" />);
  await screen.findByRole("region", { name: "实现与验收" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：实现与验收" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项标题" }), { target: { value: "本地草稿" } });
  stored[0] = { ...stored[0], title: "其他会话的新结论", revision: 2, completed: true };
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  // A remote completion may reorder the item but must retain the local draft.
  await screen.findByDisplayValue("本地草稿");
  await waitFor(() => expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(true));
  screen.getByRole("button", { name: "复制草稿" });
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  await screen.findByRole("region", { name: "其他会话的新结论" });
  expect(stored[0].title).toBe("其他会话的新结论");
});

it("keeps the draft after a failed update and allows rereading the latest record", async () => {
  stored = [first()];
  vi.mocked(api.updateTaskItem).mockRejectedValue(new Error("数据库暂不可写"));
  render(<TaskItems taskId="task" />);
  await screen.findByRole("region", { name: "实现与验收" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：实现与验收" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项描述" }), { target: { value: "保留本地修改" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByRole("alert");
  screen.getByDisplayValue("保留本地修改");
  expect(stored[0].description).toBe("保证已有数据完整");
});

it("preserves an editor when its item is deleted externally and permits canceling the orphaned draft", async () => {
  stored = [first()];
  render(<TaskItems taskId="task" />);
  await screen.findByRole("region", { name: "实现与验收" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：实现与验收" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项标题" }), { target: { value: "未保存草稿" } });
  stored = [];
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  await screen.findByText("任务事项已在其他地方修改或删除。草稿已保留，请复制后取消编辑，核对最新内容。");
  screen.getByDisplayValue("未保存草稿");
  expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(screen.queryByRole("form")).toBeNull();
  screen.getByText("暂无任务事项");
});

it("shows all items, sorts completed work last, and exposes completion through the icon toggle", async () => {
  stored = [{ ...first(), id: "done", title: "之前完成的工作", completed: true }, first()];
  render(<TaskItems taskId="task" />);
  const unfinished = await screen.findByRole("button", { name: "标记完成：实现与验收" });
  const done = screen.getByRole("button", { name: "标记未完成：之前完成的工作" });
  expect(unfinished.getAttribute("aria-pressed")).toBe("false");
  expect(done.getAttribute("aria-pressed")).toBe("true");
  const titles = () => within(screen.getByRole("region", { name: "任务事项" })).getAllByRole("heading", { level: 3 }).map(node => node.textContent);
  expect(titles()).toEqual(["实现与验收", "之前完成的工作"]);
  fireEvent.click(unfinished);
  await screen.findByRole("button", { name: "标记未完成：实现与验收" });
  expect(titles()).toEqual(["之前完成的工作", "实现与验收"]);
  fireEvent.click(screen.getByRole("button", { name: "标记未完成：实现与验收" }));
  await screen.findByRole("button", { name: "标记完成：实现与验收" });
  expect(titles()).toEqual(["实现与验收", "之前完成的工作"]);
});
