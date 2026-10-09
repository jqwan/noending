import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionSubpageTabs from "./SessionSubpageTabs";
import { api } from "../../api";
import { sessionPages, sessionTerminals } from "./sessionNavigation";
import { EVT_TERMINALS } from "../../app/routes";
import type { TerminalSummary } from "../../types";
vi.mock("../../api", () => ({ api: {
  terminalList: vi.fn(), terminalForSession: vi.fn(), launchEmbeddedResume: vi.fn(),
} }));
beforeEach(() => {
  vi.resetAllMocks(); sessionPages.clear(); sessionTerminals.clear();
  vi.mocked(api.terminalList).mockResolvedValue([]);
});
afterEach(cleanup);
const terminal = (id: string, live = true, sessionId = "s1"): TerminalSummary => ({
  terminal_id: id, session_id: sessionId, identity_revision: 1, agent: "codex", cwd: "/work",
  created_at: id, live, exit_code: null, session_title: "Session",
});
it("switches between history and detail without launching an Agent", async () => {
  const navigate = vi.fn();
  const view = render(<SessionSubpageTabs sessionId="s1" title="Session" agent="codex" navigate={navigate} />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("button", { name: "历史消息" }));
  expect(navigate).toHaveBeenLastCalledWith({ view: "session", sessionId: "s1", entry: "conversation", initialTitle: "Session", initialAgent: "codex" });
  view.rerender(<SessionSubpageTabs sessionId="s1" title="Session" agent="codex" entry="conversation" navigate={navigate} />);
  expect(screen.getByRole("button", { name: "历史消息" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "会话详情" }));
  expect(navigate).toHaveBeenLastCalledWith({ view: "session", sessionId: "s1", initialTitle: "Session", initialAgent: "codex" });
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
});
it("switches among this session's live and exited terminals and remembers the selected instance", async () => {
  vi.mocked(api.terminalList).mockResolvedValue([terminal("t1"), terminal("t2", false), terminal("other", true, "s2")]);
  const navigate = vi.fn();
  const view = render(<SessionSubpageTabs sessionId="s1" terminalId="t1" navigate={navigate} />);
  fireEvent.click(await screen.findByRole("button", { name: "切换终端" }));
  expect(screen.getAllByRole("menuitemradio")).toHaveLength(2);
  fireEvent.keyDown(screen.getByRole("button", { name: "切换终端" }), { key: "End" });
  expect(document.activeElement).toBe(screen.getAllByRole("menuitemradio")[1]);
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "切换终端" }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: /已退出/ }));
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t2" }));
  view.rerender(<SessionSubpageTabs sessionId="s1" terminalId="t2" terminalLive={false} navigate={navigate} />);
  view.rerender(<SessionSubpageTabs sessionId="s1" entry="conversation" navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: "终端 · 已退出" }));
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t2" }));
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  vi.mocked(api.terminalList).mockResolvedValue([terminal("t2", false)]);
  act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
  await waitFor(() => expect(screen.queryByRole("button", { name: "切换终端" })).toBeNull());
  expect(screen.getByRole("button", { name: "终端 · 已退出" })).toBeTruthy();
  // A native resume moves t2 elsewhere: it must no longer be selectable for s1.
  vi.mocked(api.terminalList).mockResolvedValue([terminal("t1"), terminal("t2", true, "s2")]);
  act(() => { window.dispatchEvent(new CustomEvent(EVT_TERMINALS)); });
  await waitFor(() => expect(screen.queryByRole("button", { name: "切换终端" })).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "终端 · 运行中" }));
  expect(navigate).toHaveBeenLastCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t1" }));
});
it("starts resume only on explicit terminal continue and blocks archived launches", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(null);
  vi.mocked(api.launchEmbeddedResume).mockResolvedValue({ terminal_id: "new" } as never);
  const navigate = vi.fn();
  const view = render(<SessionSubpageTabs sessionId="s1" agent="codex" archived navigate={navigate} />);
  await waitFor(() => expect(api.terminalList).toHaveBeenCalled());
  fireEvent.click(screen.getByRole("button", { name: "在终端中继续" }));
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  view.rerender(<SessionSubpageTabs sessionId="s1" agent="codex" navigate={navigate} />);
  fireEvent.click(screen.getByRole("button", { name: "在终端中继续" }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ terminalId: "new" })));
});
