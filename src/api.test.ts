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
    result: undefined, removed: { projectIds: ["p"] } },
];

it.each(cases)("publishes invalidated targets only after $command succeeds", async ({ command, args, run, result, removed }) => {
  let complete!: (value: unknown) => void;
  vi.mocked(invoke).mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const listener = vi.fn();
  window.addEventListener(EVT_REMOVED, listener);
  try {
    const request = run();
    expect(listener).not.toHaveBeenCalled();
    complete(result);
    await request;
    expect(invoke).toHaveBeenCalledWith(command, args);
    expect(listener).toHaveBeenCalledTimes(1);
    const event = listener.mock.calls[0][0] as CustomEvent<RemovedTargets>;
    expect(event.detail).toMatchObject(removed);
  } finally { window.removeEventListener(EVT_REMOVED, listener); }
});

it.each(cases)("leaves navigation valid when $command fails", async ({ run }) => {
  vi.mocked(invoke).mockRejectedValue(new Error("delete failed"));
  const listener = vi.fn();
  window.addEventListener(EVT_REMOVED, listener);
  try {
    await expect(run()).rejects.toThrow("delete failed");
    expect(listener).not.toHaveBeenCalled();
    expect([...removedTargets.sessions, ...removedTargets.workstreams, ...removedTargets.projects, ...removedTargets.terminals]).toEqual([]);
  } finally { window.removeEventListener(EVT_REMOVED, listener); }
});


it("queries persisted freshness for the requested scope without synchronizing", async () => {
  vi.mocked(invoke).mockResolvedValue({ kind: "project", id: "p" });
  await expect(api.getFreshness("project", "p")).resolves.toEqual({ kind: "project", id: "p" });
  expect(vi.mocked(invoke).mock.calls).toEqual([["get_freshness", { kind: "project", id: "p" }]]);
  vi.mocked(invoke).mockClear();
  await api.getFreshness();
  expect(vi.mocked(invoke).mock.calls).toEqual([["get_freshness", { kind: null, id: null }]]);
});

it.each(["session", "task", "project"] as const)("sends versioned summary edits and explicit clearing for %s", async kind => {
  const summary = { items: [{ id: "api.test-item-1", title: "当前结论", description: "用户修正", details: ["已核实"], title_locked: false }] };
  vi.mocked(invoke).mockResolvedValue({ revision: 3, summary });
  await expect(api.saveSummary(kind, "target", 2, summary)).resolves.toEqual({ revision: 3, summary });
  expect(invoke).toHaveBeenLastCalledWith("save_summary", { kind, id: "target", revision: 2, summary });
  await api.saveSummary(kind, "target", 3, { items: [] });
  expect(invoke).toHaveBeenLastCalledWith("save_summary", { kind, id: "target", revision: 3, summary: { items: [] } });
});
