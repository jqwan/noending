import { expect, it } from "vitest";
import { mergeMessages } from "./useCollaborationRoom";
import type { CollaborationMessage } from "../../types";

const message = (seq: number, patch: Partial<CollaborationMessage> = {}): CollaborationMessage => ({
  id: `m-${seq}`,
  room_id: "room",
  run_id: "run",
  seq,
  sender_session_id: "s1",
  body: `body ${seq}`,
  kind: "text",
  created_at: `2026-10-27T03:0${seq}:00Z`,
  ...patch,
});

it("appends a newer page without duplicating or reordering", () => {
  const merged = mergeMessages([message(1), message(2)], [message(2), message(3)]);
  expect(merged.map(m => m.seq)).toEqual([1, 2, 3]);
});

it("drops the stale tail when a newer page leaves a hole", () => {
  // 一次来了超过一页的新消息：旧页与新版之间缺口补不上，以新页为准。
  const merged = mergeMessages([message(1), message(2)], [message(40), message(41)]);
  expect(merged.map(m => m.seq)).toEqual([40, 41]);
});

it("prepends an earlier page in sequence order", () => {
  const merged = mergeMessages([message(5), message(6)], [message(3), message(4)]);
  expect(merged.map(m => m.seq)).toEqual([3, 4, 5, 6]);
});

it("keeps own messages in the single shared sequence", () => {
  // 自己的发言与成员发言共用一条房间序列，不分流成两个列表。
  const merged = mergeMessages([message(1, { sender_session_id: null })], [message(2, { sender_session_id: null })]);
  expect(merged.map(m => m.seq)).toEqual([1, 2]);
});
