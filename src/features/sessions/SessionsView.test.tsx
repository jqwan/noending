// 会话可跟踪多个任务；任务筛选匹配跟踪 ID，无跟踪筛选匹配空列表。

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionsView, { clearSessionsCache } from "./SessionsView";
import { api } from "../../api";
import { listen } from "@tauri-apps/api/event";
import { viewState } from "../../hooks/useViewState";
import type { Session } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listSessions: vi.fn(),
    listProjects: vi.fn(),
    listWorkstreams: vi.fn(),
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
    tracked_workstream_ids: [],
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

/** The 任务 filter select, found by its own options rather than by index. */
function taskFilter(): HTMLSelectElement {
  const select = screen.getByText("全部任务").closest("select");
  if (!select) throw new Error("任务筛选下拉框未渲染");
  return select as HTMLSelectElement;
}

beforeEach(() => {
  navigate.mockClear();
  clearSessionsCache();
  viewState.clear(); localStorage.clear();
  vi.mocked(api.listSessions).mockReset();
  vi.mocked(api.listProjects).mockReset().mockResolvedValue([]);
  vi.mocked(api.listWorkstreams).mockReset().mockResolvedValue([
    { id: "w1", title: "会话与 Workstream 重构", description: "", visibility: "normal", created_at: "", updated_at: "" },
  ] as never);
  vi.mocked(api.listIngestSources).mockReset().mockResolvedValue([{ id: "src", agent: "codex", path: "/p", enabled: true, origin: "default", created_at: "" }] as never);
  vi.mocked(listen).mockReset().mockResolvedValue(() => {});
});

afterEach(cleanup);

describe("Sessions 页按跟踪任务筛选", () => {
  it("筛选选项来自各 Session 的 owner，未归属是 null 而不是某个任务", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "owned", title: "有归属", tracked_workstream_ids: ["w1"] }),
      session({ id: "free", title: "没归属", tracked_workstream_ids: [] }),
    ]);
    await act(async () => { render(<SessionsView navigate={navigate} />); });

    await screen.findByText("全部任务");
    // 选中一个真实任务后，只剩 tracked_workstream_ids 包含它的那一条。
    fireEvent.change(taskFilter(), { target: { value: "w1" } });
    screen.getByText("有归属");
    expect(screen.queryByText("没归属")).toBeNull();

    // 「未归属任务」是 `tracked_workstream_ids.length === 0`，与任何任务 id 都不相等。
    fireEvent.change(taskFilter(), { target: { value: "unassigned" } });
    screen.getByText("没归属");
    expect(screen.queryByText("有归属")).toBeNull();
  });
});

it("opens the new-session page from the toolbar and empty-state action", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([]);
  render(<SessionsView navigate={navigate} />);

  await screen.findByRole("button", { name: "配置会话来源" });
  const buttons = screen.getAllByRole("button", { name: "新建会话" });
  expect(buttons).toHaveLength(2);
  buttons.forEach((button) => fireEvent.click(button));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("shows archived sessions in the same cards and filters, with continue disabled", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([session({ title: "归档会话", archived_at: "2026-10-08", tracked_workstream_ids: ["w1"] })]);
  render(<SessionsView navigate={navigate} scope="archived" />);
  await screen.findByText("归档会话");
  expect(api.listSessions).toHaveBeenCalledWith(undefined, undefined, "archived");
  expect(document.querySelectorAll(".session-card")).toHaveLength(1);
  expect((screen.getByRole("button", { name: "在终端中继续归档会话" }) as HTMLButtonElement).disabled).toBe(true);
  screen.getByRole("button", { name: "取消归档归档会话" });
  screen.getByRole("button", { name: "永久删除归档会话" });
  const bulkDeleteBtn = screen.getByRole("button", { name: "删除全部" });
  expect(bulkDeleteBtn.className).toContain("icon-button");
  expect(bulkDeleteBtn.className).toContain("danger");
  expect(bulkDeleteBtn.textContent).toBe("");
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  fireEvent.change(screen.getByRole("textbox", { name: "搜索会话" }), { target: { value: "不匹配" } });
  expect(screen.queryByText("归档会话")).toBeNull();
});


it("includes assistant sessions in the shared archived board and bulk deletion", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([
    session({ id: "assistant-archived", title: "归档助手", project_id: "assistant-project", workspace_path_id: "assistant-path", archived_at: "2026-10-11" }),
    session({ id: "regular-archived", title: "归档普通会话", project_id: "regular-project", workspace_path_id: "regular-path", archived_at: "2026-10-11" }),
  ]);
  vi.mocked(api.permanentlyDeleteSession).mockReset().mockResolvedValue({ purged: true } as never);
  await act(async () => { render(<SessionsView navigate={navigate} scope="archived" />); });
  await screen.findByText("归档助手");
  expect(screen.getByText("归档普通会话")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
  const buttons = screen.getAllByRole("button", { name: "删除全部" });
  await act(async () => { fireEvent.click(buttons[buttons.length - 1]); });
  expect(api.permanentlyDeleteSession).toHaveBeenCalledTimes(2);
  expect(api.permanentlyDeleteSession).toHaveBeenCalledWith("assistant-archived");
  expect(api.permanentlyDeleteSession).toHaveBeenCalledWith("regular-archived");
});
