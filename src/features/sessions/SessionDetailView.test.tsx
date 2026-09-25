import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionDetailView from "./SessionDetailView";
import { api } from "../../api";
import type { Session, SessionDetail, Workstream } from "../../types";

// 只覆盖「会话信息」里那棵父子会话树（§37.20）与「所属任务」这个单 Owner 入口
// （方案 §28/§29）。命令怎么走由后端测试覆盖。
vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    syncSession: vi.fn(),
    trashSession: vi.fn(),
    restoreSession: vi.fn(),
    setSessionOwnerWorkstream: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function session(id: string, over: Partial<Session> = {}): Session {
  return {
    id,
    agent: "codex",
    agent_session_id: `${id}-agent`,
    title: null,
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    owner_workstream_id: null,
    raw_path: `/${id}.jsonl`,
    parent_agent_session_id: null,
    started_at: null,
    last_activity_at: null,
    trashed_at: null,
    ...over,
  };
}

function workstream(id: string, title: string): Workstream {
  return {
    id,
    title,
    description: "",
    lifecycle: "active",
    visibility: "normal",
    created_at: "2026-09-21T00:00:00+00:00",
    updated_at: "2026-09-21T00:00:00+00:00",
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    events: [],
    owner_workstream: null,
    cursor: 0,
    processed_cursor: 0,
    raw_path_status: "present",
    workspace_path: null,
    parent: null,
    children: [],
    ...over,
  };
}

async function renderDetail(d: SessionDetail) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);
  const navigate = vi.fn();
  render(<SessionDetailView sessionId={d.session.id} navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("会话信息");
  return navigate;
}

it("links to the parent session and to each child", async () => {
  const me = session("me", { parent_agent_session_id: "p-agent" });
  const navigate = await renderDetail(detail(me, {
    parent: session("p", { title: "父会话标题", agent_session_id: "p-agent" }),
    children: [
      session("c1", { title: "子会话一" }),
      session("c2", { title: "子会话二" }),
    ],
  }));

  fireEvent.click(screen.getByRole("button", { name: "父会话标题" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "p" });

  fireEvent.click(screen.getByRole("button", { name: "子会话一" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c1" });

  fireEvent.click(screen.getByRole("button", { name: "子会话二" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c2" });
});

it("says the parent is missing instead of pretending there is none", async () => {
  const me = session("me", { parent_agent_session_id: "ghost-agent" });
  await renderDetail(detail(me, { parent: null }));

  // 转录记了父会话、但我们从没发现过那一条：字段在，链接不在。
  screen.getByText("父会话");
  screen.getByText(/不在 NoEnding 库里/);
  screen.getByText(/ghost-agent/);
  expect(screen.queryByRole("button", { name: /未命名会话/ })).toBeNull();
});

it("marks a trashed child but still links to it", async () => {
  const me = session("me");
  const navigate = await renderDetail(detail(me, {
    children: [session("c1", { title: "子会话一", trashed_at: "2026-09-24T00:00:00+00:00" })],
  }));

  const link = screen.getByRole("button", { name: /子会话一/ });
  expect(link.textContent).toContain("回收站");
  fireEvent.click(link);
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c1" });
});

it("shows no session-tree fields for a session that has neither", async () => {
  await renderDetail(detail(session("me"), { children: [] }));

  expect(screen.queryByText("父会话")).toBeNull();
  expect(screen.queryByText(/^子会话/)).toBeNull();
});

// ---------------- 所属任务（单 Owner，方案 §28/§29） ----------------

it("shows the one owner workstream and links to it", async () => {
  const owner = workstream("w1", "会话与 Workstream 重构");
  const navigate = await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), {
    owner_workstream: owner,
  }));

  screen.getByText("所属任务");
  const link = screen.getByRole("button", { name: owner.title });
  fireEvent.click(link);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });
  // 只有一个所属任务：没有「添加」多任务入口。
  expect(screen.queryByRole("button", { name: /添加/ })).toBeNull();
});

it("marks an unowned session and offers 选择 instead of 添加", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("所属任务");
  screen.getByText("未归属任务");
  screen.getByRole("button", { name: "选择" });
  expect(screen.queryByRole("button", { name: "更改" })).toBeNull();
});

it("replaces the owner through a single-choice modal", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner, workstream("w2", "新任务")]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  const dialog = await screen.findByRole("dialog", { name: "选择所属任务" });
  // 单选：一次只能选一个。
  const radios = dialog.querySelectorAll('input[type="radio"]');
  expect(radios).toHaveLength(3);
  fireEvent.click(screen.getByText("新任务"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", "w2"));
});

it("clears the owner by choosing 未归属", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  await screen.findByRole("dialog", { name: "选择所属任务" });
  fireEvent.click(screen.getByText("未归属"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", null));
});
