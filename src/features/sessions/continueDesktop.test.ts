import { removedTargets } from "../../app/removedTargets";
import { beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import { sessionTerminals } from "./sessionNavigation";
import { continueSessionTerminal } from "./continueDesktop";
import type { TerminalSummary } from "../../types";

vi.mock("../../api", () => ({ api: {
  terminalList: vi.fn(), launchEmbeddedResume: vi.fn(), terminalReconnect: vi.fn(), getSessionTerminalStates: vi.fn(),
} }));
const session = { id: "s1", title: "Session", agent: "codex" as const };
const terminal = (live: boolean): TerminalSummary => ({
  terminal_id: "old", session_id: "s1", initial_workstream_ids: [], identity_revision: 1, agent: "codex",
  live, exit_code: live ? null : 0, cwd: "/work", created_at: "", session_title: "Session",
});
beforeEach(() => {
  Object.values(removedTargets).forEach(ids => ids.clear());
  vi.resetAllMocks();
  sessionTerminals.clear();
  vi.mocked(api.terminalList).mockResolvedValue([]);
  vi.mocked(api.getSessionTerminalStates).mockResolvedValue({ s1: { available: true, reason: null } });
  vi.mocked(api.launchEmbeddedResume).mockResolvedValue({
    terminal_id: "new", launched_via: "embedded", command_line: "", note: "", });
  vi.mocked(api.terminalReconnect).mockResolvedValue({
    terminal_id: "reconnected", launched_via: "embedded", command_line: "", note: "", });
});
it("reuses a live terminal even when new resume prerequisites are unavailable", async () => {
  vi.mocked(api.terminalList).mockResolvedValue([terminal(true)]);
  vi.mocked(api.getSessionTerminalStates).mockResolvedValue({ s1: { available: false, reason: "源文件已移除" } });
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.terminalReconnect).not.toHaveBeenCalled();
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "old" }));
});
it("starts a fresh terminal when no instance exists", async () => {
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate);
  expect(api.launchEmbeddedResume).toHaveBeenCalledWith("s1");
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "new" }));
});
it("reconnects only the explicitly selected exited instance and includes expected identity", async () => {
  vi.mocked(api.terminalList).mockResolvedValue([terminal(false), { ...terminal(true), terminal_id: "live" }]);
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate, "old");
  expect(api.terminalReconnect).toHaveBeenCalledWith("old", "s1");
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "reconnected" }));
});
it("preserves an exited terminal when reconnect fails", async () => {
  vi.mocked(api.terminalList).mockResolvedValue([terminal(false)]);
  vi.mocked(api.terminalReconnect).mockRejectedValue(new Error("spawn failed"));
  const navigate = vi.fn();
  await expect(continueSessionTerminal(session, navigate)).rejects.toThrow("spawn failed");
  expect(navigate).not.toHaveBeenCalled();
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
});
it.each(["已归档", "CLI 不存在", "工作目录不存在", "源会话不存在"])("blocks startup and reconnection when prerequisites fail: %s", async reason => {
  vi.mocked(api.getSessionTerminalStates).mockResolvedValue({ s1: { available: false, reason } });
  for (const rows of [[], [terminal(false)]]) {
    vi.mocked(api.terminalList).mockResolvedValue(rows);
    await expect(continueSessionTerminal(session, vi.fn())).rejects.toThrow(reason);
  }
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.terminalReconnect).not.toHaveBeenCalled();
});
it("prefers a remembered live instance, otherwise another live instance, and refuses stale explicit identity", async () => {
  sessionTerminals.set("s1", "selected");
  vi.mocked(api.terminalList).mockResolvedValue([{ ...terminal(true), terminal_id: "selected" }, terminal(true)]);
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate);
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ terminalId: "selected" }));
  vi.mocked(api.terminalList).mockResolvedValue([{ ...terminal(false), terminal_id: "selected" }, terminal(true)]);
  await continueSessionTerminal(session, navigate);
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ terminalId: "old" }));
  vi.mocked(api.terminalList).mockResolvedValue([{ ...terminal(true), terminal_id: "selected", session_id: "other" }]);
  await expect(continueSessionTerminal(session, navigate, "selected")).rejects.toThrow("已切换会话");
  expect(api.terminalReconnect).not.toHaveBeenCalled();
});
