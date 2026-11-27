import { expect, it } from "vitest";
import { memberStatus } from "./collaborationState";
import type { CollaborationMemberState } from "../../types";

const NOW = Date.parse("2026-10-27T04:00:00Z");
const member = (patch: Partial<CollaborationMemberState> = {}): CollaborationMemberState => ({
  title: null, agent: null, project_id: null,
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

it("only marks listening members with old unread messages as unresponsive", () => {
  expect(memberStatus(member(), NOW).label).toBe("暂未响应");
  expect(memberStatus(member({ awaiting_since: "2026-10-27T03:59:30Z" }), NOW).label).toBe("等待消息");
  expect(memberStatus(member({ awaiting_since: null, unread_count: 0 }), NOW).label).toBe("等待消息");
  expect(memberStatus(member({ phase: "processing" }), NOW).label).toBe("处理中");
});

it("shows the connection and listening state ahead of an earlier wake result", () => {
  const cases = [
    [{ phase: null }, "仅连接"],
    [{ connection_state: "disconnected" }, "连接中断"],
    [{ connection_state: "pending", wake_status: "pending" }, "等待加入"],
    [{ connection_state: "pending", wake_status: "failed" }, "打开失败"],
    [{ wake_status: "failed", awaiting_since: null }, "等待消息"],
  ] satisfies [Partial<CollaborationMemberState>, string][];
  for (const [state, label] of cases) expect(memberStatus(member(state), NOW).label).toBe(label);
});

it("expires the observed connection when no further heartbeat event arrives", () => {
  expect(memberStatus(member({ last_heartbeat_at: new Date(NOW - 30_000).toISOString() }), NOW).label).toBe("暂未响应");
  expect(memberStatus(member({ last_heartbeat_at: new Date(NOW - 31_000).toISOString() }), NOW).label).toBe("连接中断");
});
