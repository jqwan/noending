import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { api } from "./api";
import { EVT_REMOVED, removedTargets, type RemovedTargets } from "./app/removedTargets";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  Object.values(removedTargets).forEach(ids => ids.clear());
});

const cases = [
  { command: "permanently_delete_session", args: { sessionId: "s" }, run: () => api.permanentlyDeleteSession("s"),
    result: { purged: true, terminal_ids: ["t1", "t2"] }, removed: { sessionIds: ["s"], terminalIds: ["t1", "t2"] } },
  { command: "delete_workstream_permanently", args: { workstreamId: "w" }, run: () => api.deleteWorkstreamPermanently("w"),
    result: undefined, removed: { workstreamIds: ["w"] } },
  { command: "delete_project_permanently", args: { projectId: "p" }, run: () => api.deleteProjectPermanently("p"),
    result: { warnings: [] }, removed: { projectIds: ["p"] } },
];

it.each(cases)("publishes invalidation on $command success and retains targets on failure", async ({ command, args, run, result, removed }) => {
  const listener = vi.fn();
  window.addEventListener(EVT_REMOVED, listener);
  try {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("delete failed"));
    await expect(run()).rejects.toThrow("delete failed");
    expect(listener).not.toHaveBeenCalled();
    expect(Object.values(removedTargets).flatMap(ids => [...ids])).toEqual([]);

    let complete!: (value: unknown) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise(resolve => { complete = resolve; }));
    const request = run();
    expect(listener).not.toHaveBeenCalled();
    complete(result);
    await request;
    expect(invoke).toHaveBeenLastCalledWith(command, args);
    expect(listener).toHaveBeenCalledOnce();
    const event = listener.mock.calls[0][0] as CustomEvent<RemovedTargets>;
    expect(event.detail).toMatchObject(removed);
  } finally { window.removeEventListener(EVT_REMOVED, listener); }
});

it("queries active operation presence without submitting work", async () => {
  const result = [{id:"queued",kind:"sync",state:"queued",targets:[{kind:"session",id:"one"}],agent:null}];
  vi.mocked(invoke).mockResolvedValue(result);
  await expect(api.getOperationStatus()).resolves.toEqual(result);
  expect(vi.mocked(invoke).mock.calls).toEqual([["get_operation_status"]]);
});

it("uses independent task work item IDs and revisions over IPC", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  await api.listTaskItems("task");
  await api.createTaskItem("task", { title: "验收", description: "已有数据完整" });
  await api.updateTaskItem("task", "work", 3, { details: ["回归通过"] });
  await api.deleteTaskItem("task", "work", 4);
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["list_task_items", { taskId: "task" }],
    ["create_task_item", { taskId: "task", content: { title: "验收", description: "已有数据完整" } }],
    ["update_task_item", { taskId: "task", itemId: "work", revision: 3, patch: { details: ["回归通过"] } }],
    ["delete_task_item", { taskId: "task", itemId: "work", revision: 4 }],
  ]);
});

it("cleans directory registrations without invalidating the surviving project", async () => {
  vi.mocked(invoke).mockResolvedValue(2);
  const listener = vi.fn();
  window.addEventListener(EVT_REMOVED, listener);
  try {
    await expect(api.cleanMissingProjectPaths("project")).resolves.toBe(2);
    expect(invoke).toHaveBeenCalledWith("clean_missing_project_paths", { projectId: "project" });
    expect(listener).not.toHaveBeenCalled();
    expect(removedTargets.projects.has("project")).toBe(false);
  } finally { window.removeEventListener(EVT_REMOVED, listener); }
});
