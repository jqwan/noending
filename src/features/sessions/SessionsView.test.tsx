import { invalidateTargets, removedTargets } from "../../app/removedTargets";

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionsView, { clearSessionsCache } from "./SessionsView";
import { api } from "../../api";
import { listen } from "@tauri-apps/api/event";
import { viewState } from "../../hooks/useViewState";
import type { Session } from "../../types";

vi.mock("../../api", () => ({
  api: { matchObjects: vi.fn().mockResolvedValue([]),
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    listSessions: vi.fn(),
    listProjects: vi.fn(),
    listIngestSources: vi.fn(),
    archiveSession: vi.fn(),
    restoreSession: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
    getAgentStatus: vi.fn().mockResolvedValue({}),
    continueSessionDesktop: vi.fn().mockResolvedValue({ uri: "x://y", note: "已打开" }),
    terminalList: vi.fn().mockResolvedValue([]),
  },
}));

// 组件挂载时订阅刷新事件；测试环境没有 Tauri IPC。
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const navigate = vi.fn();

function session(over: Partial<Session> = {}): Session {
  return {
    id: "s1",
    agent: "codex",
    root_agent_session_id: "a1",
    title: "会话",
    cwd: "/repo",
    workspace_path_id: null,
    project_id: null,
    forked_from_session_id: null,
    started_at: "2026-09-01T00:00:00Z",
    last_activity_at: "2026-09-02T00:00:00Z",
    last_conversation_at: "2026-09-02T00:00:00Z",
    archived_at: null,
        source_kind: "codex_rollout",
        source_path: "/tmp/rollout.jsonl",
        metadata: {},
        source_file_identity: "identity",
        source_generation: 1,
        source_byte_offset: 100,
        source_last_seen_size: 100,
        source_mtime: null,
        source_prefix_hash: "",
        source_tail_hash: "",
        fact_generation: 1,
        latest_message_seq: 2,

    ...over,
  };
}

beforeEach(() => {
  Object.values(removedTargets).forEach(ids => ids.clear());
  navigate.mockClear();
  vi.mocked(api.matchObjects).mockReset().mockResolvedValue([]);
  clearSessionsCache();
  viewState.clear(); localStorage.clear();
  vi.mocked(api.listSessions).mockReset();
  vi.mocked(api.listProjects).mockReset().mockResolvedValue([]);
  vi.mocked(api.listIngestSources).mockReset().mockResolvedValue([{ id: "src", agent: "codex", path: "/p", origin: "default", created_at: "", exists: true }] as never);
  vi.mocked(listen).mockReset().mockResolvedValue(() => {});
});

afterEach(cleanup);

it("offers source configuration when a directory is missing and can start an independent session", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([]);
  vi.mocked(api.listIngestSources).mockResolvedValue([
    { id: "missing", agent: "codex", path: "/missing/history", origin: "default", exists: false },
  ]);
  render(<SessionsView navigate={navigate} />);
  await screen.findByText("来源目录不存在：/missing/history");
  fireEvent.click(screen.getByRole("button", { name: "配置会话来源" }));
  expect(navigate).toHaveBeenLastCalledWith({ view: "agents" });
  navigate.mockClear();
  const buttons = screen.getAllByRole("button", { name: "新建会话" });
  expect(buttons).toHaveLength(2);
  buttons.forEach((button) => fireEvent.click(button));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("loads archived sessions and keeps search and archive actions available", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([session({ title: "归档会话", archived_at: "2026-10-08" })]);
  render(<SessionsView navigate={navigate} scope="archived" />);
  await screen.findByText("归档会话");
  expect(api.listSessions).toHaveBeenCalledWith(undefined, undefined, "archived");
  screen.getByRole("button", { name: "取消归档归档会话" });
  screen.getByRole("button", { name: "永久删除归档会话" });
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  fireEvent.change(screen.getByRole("textbox", { name: "搜索会话" }), { target: { value: "不匹配" } });
  await screen.findByText("没有符合当前筛选条件的会话");
  expect(screen.queryByText("归档会话")).toBeNull();
});


it("bulk-deletes the archived sessions after confirmation", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([
    session({ id: "archived-one", title: "归档会话一", archived_at: "2026-10-11" }),
    session({ id: "archived-two", title: "归档会话二", archived_at: "2026-10-11" }),
  ]);
  vi.mocked(api.permanentlyDeleteSession).mockReset().mockResolvedValue({ purged: true } as never);
  await act(async () => { render(<SessionsView navigate={navigate} scope="archived" />); });
  await screen.findByText("归档会话一");
  fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
  expect(api.permanentlyDeleteSession).not.toHaveBeenCalled();
  const buttons = screen.getAllByRole("button", { name: "删除全部" });
  await act(async () => { fireEvent.click(buttons[buttons.length - 1]); });
  expect(vi.mocked(api.permanentlyDeleteSession).mock.calls.map(([id]) => id).sort()).toEqual([
    "archived-one", "archived-two",
  ]);
});

it("immediately removes a deleted session and ignores a stale board refresh", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([session({ id: "gone", title: "删除会话" }), session({ id: "kept", title: "保留会话" })]);
  render(<SessionsView navigate={navigate} />);
  await screen.findByText("删除会话");
  act(() => invalidateTargets({ sessionIds: ["gone"] }));
  expect(screen.queryByText("删除会话")).toBeNull();
  await act(async () => {});
  expect(screen.queryByText("删除会话")).toBeNull();
  expect(screen.getByText("保留会话")).toBeTruthy();
});

it("matches all session identities before the visible card limit", async () => {
  const sessions = Array.from({length:125}, (_,n)=>session({id:`s-${n}`, title:`Session ${n}`}));
  vi.mocked(api.listSessions).mockResolvedValue(sessions);
  vi.mocked(api.matchObjects).mockResolvedValue(["s-124"]);
  render(<SessionsView navigate={navigate} />);
  await screen.findByText("Session 0");
  expect(screen.queryByText("Session 124")).toBeNull();
  fireEvent.change(screen.getByRole("textbox",{name:"搜索会话"}),{target:{value:"native-id"}});
  await screen.findByText("Session 124");
  expect(screen.queryByText("Session 0")).toBeNull();
  expect(api.matchObjects).toHaveBeenCalledWith("session","native-id", sessions.map(session => session.id));
});
