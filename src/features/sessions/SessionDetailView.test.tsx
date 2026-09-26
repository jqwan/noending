import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionDetailView from "./SessionDetailView";
import { api } from "../../api";
import type {
  Session,
  SessionAggregateStats,
  SessionContextView,
  SessionDetail,
  SessionMember,
  SessionMessage,
  SessionMemberStats,
  Workstream,
} from "../../types";

// 只覆盖重构后的详情页：执行信息（聚合统计 + 成员树）、源会话状态、fork 链接、
// 回收站横幅的永久删除门槛、「所属任务」单 Owner 入口，以及 Context 面板。
vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    getSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      fields: null,
      revision: 0,
      ingest_generation: 0,
      processed_through_seq: 0,
      latest_message_seq: 0,
      updated_at: null,
      pending: false,
    }),
    updateSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      status: "updated",
      revision: 1,
    }),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
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
    root_agent_session_id: `${id}-agent`,
    title: null,
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    owner_workstream_id: null,
    forked_from_session_id: null,
    started_at: null,
    last_activity_at: null,
    last_conversation_at: null,
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

function stats(over: Partial<SessionAggregateStats> = {}): SessionAggregateStats {
  return {
    member_count: 1,
    child_count: 0,
    side_count: 0,
    max_depth: 0,
    tool_call_count: 0,
    tool_error_count: 0,
    compaction_count: 0,
    side_activity_count: 0,
    input_tokens: null,
    output_tokens: null,
    cached_tokens: null,
    reasoning_tokens: null,
    cost: null,
    ...over,
  };
}

function member(
  sessionId: string,
  sourceMemberId: string,
  relation: SessionMember["relation"],
  parentSourceMemberId: string | null,
  over: Partial<SessionMember> & { stats?: SessionMemberStats | null } = {},
): SessionMember & { stats: SessionMemberStats | null } {
  const { stats: memberStats = null, ...rest } = over;
  return {
    id: `${sessionId}-${sourceMemberId}`,
    session_id: sessionId,
    agent: "codex",
    source_member_id: sourceMemberId,
    relation,
    parent_source_member_id: parentSourceMemberId,
    source_kind: "codex_thread",
    source_path: `/sources/${sourceMemberId}.jsonl`,
    cwd: null,
    started_at: null,
    last_activity_at: null,
    metadata: {},
    stats: memberStats,
    ...rest,
  };
}

function message(
  sessionId: string,
  sequence: number,
  role: SessionMessage["role"],
  content: string,
): SessionMessage {
  return {
    id: `m${sequence}`,
    session_id: sessionId,
    member_id: "root",
    sequence,
    role,
    content,
    ts: null,
    source_message_id: null,
    source_generation: 0,
    source_position: "",
    source_identity_hash: "",
    provider: null,
    model: null,
    raw_ref: "",
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    messages: [],
    owner_workstream: null,
    workspace_path: null,
    members: [member(me.id, `${me.id}-root`, "root", null)],
    stats: stats(),
    ingested_message_sequence: 0,
    processed_message_sequence: 0,
    root_source_status: "present",
    can_resume: true,
    can_permanently_delete: false,
    forked_from: null,
    ...over,
  };
}

async function renderDetail(d: SessionDetail) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);
  const navigate = vi.fn();
  const view = render(<SessionDetailView sessionId={d.session.id} navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("会话信息");
  return { navigate, container: document.body, rerender: view.rerender };
}

// 执行信息

it("shows aggregate execution stats and an expandable member tree", async () => {
  const me = session("me");
  await renderDetail(detail(me, {
    members: [
      member(me.id, `${me.id}-root`, "root", null),
      member(me.id, "child-src-1", "child", `${me.id}-root`, {
        stats: { member_id: "c1", tool_call_count: 12, tool_error_count: 2, compaction_count: 1 } as SessionMemberStats,
      }),
      member(me.id, "side-src-1", "side", `${me.id}-root`),
    ],
    stats: stats({
      member_count: 3,
      child_count: 1,
      side_count: 1,
      max_depth: 1,
      tool_call_count: 12,
      tool_error_count: 2,
      compaction_count: 1,
    }),
  }));

  // 聚合统计常驻。
  const body = document.body.textContent ?? "";
  expect(body).toContain("成员 3 · 子 1 · 边 1 · 最大深度 1");
  expect(body).toContain("工具调用 12 · 失败 2 · 压缩 1");

  // 成员默认收起，点开才出现。
  expect(screen.queryByText("child-src-1")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "展开成员（3）" }));

  screen.getByText("child-src-1");
  screen.getByText("side-src-1");
  // 关系标签：根 / 子 / 边执行。
  expect(screen.getAllByText("根")).toHaveLength(1);
  screen.getByText("子");
  screen.getByText("边执行");
  // 成员自己的计数只在有数据时出现。
  expect(document.body.textContent).toContain("工具 12 · 失败 2 · 压缩 1");

  // 成员是执行信息，不是可进入的其他会话页面。
  expect(screen.queryByRole("button", { name: /child-src-1/ })).toBeNull();
  expect(screen.queryByRole("button", { name: /side-src-1/ })).toBeNull();
});

it("shows tool / token / cost rows only when the data is present", async () => {
  const me = session("me");
  await renderDetail(detail(me, { stats: stats() }));

  let body = document.body.textContent ?? "";
  expect(body).not.toContain("Tokens");
  expect(body).not.toContain("成本");
  expect(body).not.toContain("工具调用");

  cleanup();
  await renderDetail(detail(me, {
    stats: stats({
      tool_call_count: 4,
      input_tokens: 1000,
      output_tokens: 200,
      cost: 0.5,
    }),
  }));

  body = document.body.textContent ?? "";
  expect(body).toContain("工具调用 4");
  expect(body).toContain("Tokens 输入 1,000 · 输出 200");
  expect(body).toContain("成本 0.5");
});

// 源会话

it("shows the root member's source as 源会话 without status noise when present", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("源会话");
  screen.getByText(/sources\/me-root\.jsonl/);
  expect(screen.queryByText("源会话已不存在")).toBeNull();
  expect(screen.queryByText("无法确认源会话状态")).toBeNull();
});

it("warns and disables resume when the root source is missing", async () => {
  await renderDetail(detail(session("me"), { root_source_status: "missing", can_resume: false }));

  screen.getByText("源会话已不存在");
  const resume = screen.getByRole("button", { name: "继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(true);
  expect(resume.title).toContain("源会话已不存在");
});

it("warns when the root source status is unavailable", async () => {
  await renderDetail(detail(session("me"), { root_source_status: "unavailable", can_resume: false }));

  screen.getByText("无法确认源会话状态");
  const resume = screen.getByRole("button", { name: "继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(true);
});

it("treats a missing root member as unavailable even when the verdict says present", async () => {
  await renderDetail(detail(session("me"), { members: [], root_source_status: "present" }));

  screen.getByText(/没有 Root 成员记录/);
  screen.getByText("无法确认源会话状态");
});

// 消息

it("renders only the user/assistant conversation", async () => {
  await renderDetail(detail(session("me"), {
    messages: [
      message("me", 1, "user", "帮我看看这个报错"),
      message("me", 2, "assistant", "好的，我在看"),
    ],
  }));

  screen.getByText("帮我看看这个报错");
  screen.getByText("好的，我在看");
});

// Fork

it("links to the session it forked from", async () => {
  const me = session("me", { forked_from_session_id: "orig" });
  const { navigate } = await renderDetail(detail(me, {
    forked_from: session("orig", { title: "原始会话" }),
  }));

  fireEvent.click(screen.getByRole("button", { name: "分叉自：原始会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "orig" });
});

it("names the missing fork source instead of hiding the provenance", async () => {
  const me = session("me", { forked_from_session_id: "ghost" });
  await renderDetail(detail(me));

  screen.getByText(/来源会话不在 NoEnding 库里/);
  screen.getByText(/ghost/);
});

it("shows no fork field for a session that is not a fork", async () => {
  await renderDetail(detail(session("me")));

  expect(screen.queryByText(/分叉自/)).toBeNull();
});

// 回收站横幅

it("offers permanent delete in the trash banner only when allowed", async () => {
  await renderDetail(detail(session("me", { trashed_at: "2026-09-24T00:00:00+00:00" }), {
    can_resume: false,
    root_source_status: "missing",
    can_permanently_delete: true,
  }));

  screen.getByRole("button", { name: "永久删除…" });
  expect(screen.queryByText(/永久删除不可用/)).toBeNull();
});

it("explains why permanent delete is unavailable while trashed", async () => {
  await renderDetail(detail(session("me", { trashed_at: "2026-09-24T00:00:00+00:00" }), {
    root_source_status: "present",
    can_permanently_delete: false,
  }));

  expect(screen.queryByRole("button", { name: "永久删除…" })).toBeNull();
  screen.getByText(/永久删除不可用（Root 源仍存在或无法确认）/);
});

// 所属任务（单 Owner）

it("shows the one owner workstream and links to it", async () => {
  const owner = workstream("w1", "会话与 Workstream 重构");
  const { navigate } = await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), {
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

// Context 面板

function contextView(over: Partial<SessionContextView> = {}): SessionContextView {
  return {
    session_id: "me",
    fields: null,
    revision: 0,
    ingest_generation: 0,
    processed_through_seq: 0,
    latest_message_seq: 0,
    updated_at: null,
    pending: false,
    ...over,
  };
}

it("offers 生成摘要 only when there are messages and no summary yet", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  screen.getByText("Context");
  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  // 更新成功后重新拉取 Context 与详情。
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
});

it("keeps the summary read-only and shows 更新摘要 while there is a pending increment", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "正在重构前端入口",
      decisions: ["删掉 Context Intelligence 开关"],
      open_questions: [],
      next_steps: ["补测试"],
    },
    pending: true,
    revision: 3,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "继续")],
  }));

  screen.getByText(/正在重构前端入口/);
  screen.getByText(/删掉 Context Intelligence 开关/);
  screen.getByText(/补测试/);
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
});

it("hides the update button when there is nothing pending", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "已完成",
      decisions: [],
      open_questions: [],
      next_steps: [],
    },
    pending: false,
    revision: 1,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "好了")],
  }));

  expect(screen.queryByRole("button", { name: "更新摘要" })).toBeNull();
  expect(screen.queryByRole("button", { name: "生成摘要" })).toBeNull();
});

it("shows structured Context failure details and copies the operation id", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue(
    { code: "stale_snapshot", message: "内容已变化，请重新更新", operation_id: "123e4567-e89b-12d3-a456-426614174000" },
  );
  const originalClipboard = navigator.clipboard;
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await screen.findByText("内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000");
  fireEvent.click(screen.getByRole("button", { name: "复制错误详情" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "内容已变化，请重新更新\n错误代码：stale_snapshot\n操作 ID：123e4567-e89b-12d3-a456-426614174000",
  ));
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
});

it("keeps an update error visible across background sync refreshes", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  window.dispatchEvent(new Event("noending:sync"));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
  expect(screen.getByText(failure)).toBeTruthy();
});

it("clears a session's update error when the same detail view navigates to another session", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  const { navigate, rerender } = await renderDetail(detail(session("session-a"), {
    messages: [message("session-a", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  rerender(<SessionDetailView sessionId="session-b" navigate={navigate} goBack={vi.fn()} />);
  await waitFor(() => expect(screen.queryByText(failure)).toBeNull());
});
