import { expect, it } from "vitest";
import { joinPrompt, memberStatus } from "./collaborationState";
import type { CollaborationMemberState } from "../../types";

const NOW = Date.parse("2026-10-27T04:00:00Z");
const member = (patch: Partial<CollaborationMemberState> = {}): CollaborationMemberState => ({
  session_id: "s1",
  wake_method: "terminal",
  wake_status: "opened",
  connection_state: "connected",
  phase: "listening",
  phase_since: null,
  last_heartbeat_at: "2026-10-27T03:59:50Z",
  last_read_at: null,
  last_read_seq: 4,
  awaiting_since: "2026-10-27T03:55:00Z",
  unread_count: 1,
  error: null,
  ...patch,
});

it("flags a listening member as unresponsive only after the earliest unread waited past two minutes", () => {
  expect(memberStatus(member(), NOW).label).toBe("暂未响应");
  expect(memberStatus(member({ awaiting_since: "2026-10-27T03:59:30Z" }), NOW).label).toBe("正在监听");
  // 没有待查收的消息时，久未活动也不该被判为无回应。
  expect(memberStatus(member({ awaiting_since: null, unread_count: 0 }), NOW).label).toBe("正在监听");
});

it("keeps processing independent of the two-minute rule", () => {
  expect(memberStatus(member({ phase: "processing" }), NOW).label).toBe("处理中");
});

it("does not read a live heartbeat as the model being ready to answer", () => {
  // 连接有效但没有监听阶段：只说「已连接」，不写「正在监听」也不写工作正常。
  expect(memberStatus(member({ phase: null }), NOW).label).toBe("已连接");
  expect(memberStatus(member({ connection_state: "disconnected" }), NOW).label).toBe("连接中断");
  expect(memberStatus(member({ connection_state: "pending", wake_status: "pending" }), NOW).label).toBe("等待加入");
  expect(memberStatus(member({ connection_state: "pending", wake_status: "failed" }), NOW).label).toBe("唤醒失败");
});

it("expires the observed connection when no further heartbeat event arrives", () => {
  expect(memberStatus(member({ last_heartbeat_at: new Date(NOW - 30_000).toISOString() }), NOW).label).toBe("暂未响应");
  expect(memberStatus(member({ last_heartbeat_at: new Date(NOW - 31_000).toISOString() }), NOW).label).toBe("连接中断");
});

it("shows an established connection after a manual join even when the earlier wake failed", () => {
  expect(memberStatus(member({ wake_status: "failed", awaiting_since: null }), NOW).label).toBe("正在监听");
});

it("writes a join prompt with the run coordinates and no message body", () => {
  const text = joinPrompt({
    taskId: "task-1",
    taskTitle: "跨会话聊天室开发",
    roomId: "room-1",
    runId: "run-1",
    selfSessionId: "s1",
    coordinator: true,
  });
  expect(text).toContain("task-1");
  expect(text).toContain("room-1");
  expect(text).toContain("run-1");
  expect(text).toContain("负责人");
  expect(text).toContain("join");
});
