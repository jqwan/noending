import { beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import { continueSessionTerminal } from "./continueDesktop";
import type { TerminalSummary } from "../../types";

vi.mock("../../api", () => ({ api: { terminalForSession: vi.fn(), launchEmbeddedResume: vi.fn() } }));
const session = { id: "s1", title: "Session", agent: "codex" as const };
const terminal = (live: boolean): TerminalSummary => ({
  terminal_id: "old", session_id: "s1", identity_revision: 1, agent: "codex",
  live, exit_code: live ? null : 0, cwd: "/work", created_at: "", session_title: "Session",
});
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.launchEmbeddedResume).mockResolvedValue({
    terminal_id: "new", launched_via: "embedded", command_line: "", note: "", launch_intent_id: null,
  });
});
it("reuses a live terminal", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(terminal(true));
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "old" }));
});
it.each([null, terminal(false)])("starts a fresh terminal when the old one is absent or exited: %s", async (old) => {
  vi.mocked(api.terminalForSession).mockResolvedValue(old);
  const navigate = vi.fn();
  await continueSessionTerminal(session, navigate);
  expect(api.launchEmbeddedResume).toHaveBeenCalledWith("s1");
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "new" }));
  expect(api.terminalForSession).toHaveBeenCalledTimes(1);
});
it("never falls back to an old terminal when launching fails", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(terminal(false));
  vi.mocked(api.launchEmbeddedResume).mockRejectedValue(new Error("spawn failed"));
  const navigate = vi.fn();
  await expect(continueSessionTerminal(session, navigate)).rejects.toThrow("spawn failed");
  expect(navigate).not.toHaveBeenCalled();
});
it("does not reopen an archived session", async () => {
  await expect(continueSessionTerminal({ ...session, archived_at: "archived" }, vi.fn())).rejects.toThrow("已归档");
  expect(api.terminalForSession).not.toHaveBeenCalled();
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
});
