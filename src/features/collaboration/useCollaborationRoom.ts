import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { CollaborationMessage, CollaborationRoomView } from "../../types";

const PAGE = 30;

/**
 * 合并同一房间的消息：按 seq 去重升序。
 * 新一页与已有序列之间出现空档（一次来了超过一页）时，旧页不再可信，以新页为准；
 * `replaced` 通知调用方连"更早一页"的游标一起换掉，否则下一次向前翻页会跳过中间历史。
 */
export function mergeMessages(
  old: CollaborationMessage[],
  page: CollaborationMessage[],
): { items: CollaborationMessage[]; replaced: boolean } {
  if (!page.length) return { items: old, replaced: false };
  const maxOld = old.length ? old[old.length - 1].seq : 0;
  const minNew = page[0].seq;
  if (old.length && minNew > maxOld + 1) return { items: page, replaced: true };
  const byId = new Map(old.map(m => [m.id, m]));
  for (const m of page) byId.set(m.id, m);
  return { items: [...byId.values()].sort((a, b) => a.seq - b.seq), replaced: false };
}

interface ChangedEvent {
  task_id?: string;
  room_id?: string;
  run_id?: string;
  /** 只有这四类；心跳走 member，只刷房间与成员状态。 */
  category?: "room" | "run" | "member" | "message";
}

/**
 * 协作室读取：房间状态与消息分开刷新。
 *
 * member（心跳、连接、阅读位置）与 room（配置）只重读房间，不动消息列表；
 * run 与 message 才追加消息。opaque cursor 由后端给出方向，前端只回传。
 */
export function useCollaborationRoom(taskId: string) {
  const [room, setRoom] = useState<CollaborationRoomView | null>(null);
  const [messages, setMessages] = useState<CollaborationMessage[]>([]);
  const [earlierCursor, setEarlierCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const roomToken = useRef(0);
  const msgToken = useRef(0);
  const messagesRef = useRef<CollaborationMessage[]>([]);
  messagesRef.current = messages;
  const roomId = room?.room?.id ?? null;
  const current = useRef<{ roomId: string | null; earlier: string | null }>({ roomId: null, earlier: null });
  current.current.roomId = roomId;
  current.current.earlier = earlierCursor;

  const refreshRoom = useCallback(async () => {
    const token = ++roomToken.current;
    try {
      const next = await api.collaborationRoomGet(taskId);
      if (roomToken.current === token) {
        setRoom(next);
        setError("");
      }
      return next;
    } catch (e) {
      if (roomToken.current === token) setError(String(e));
      return null;
    }
  }, [taskId]);

  /** 追加最近一页；`reset` 用于切换房间或首屏。 */
  const loadLatest = useCallback(async (id: string, reset = false) => {
    const token = ++msgToken.current;
    try {
      const page = await api.collaborationRoomMessages(id, null, null, PAGE);
      if (msgToken.current !== token) return;
      const merged = reset ? { items: page.items, replaced: true } : mergeMessages(messagesRef.current, page.items);
      setMessages(merged.items);
      if (merged.replaced) setEarlierCursor(page.next_cursor);
      else setEarlierCursor(cursor => cursor ?? page.next_cursor);
      setError("");
    } catch (e) {
      if (msgToken.current === token) setError(String(e));
    }
  }, []);

  /** 向前翻页：历史查询的 next_cursor 只表示更早一页，wait 的游标才指向后续页。 */
  const loadEarlier = useCallback(async () => {
    const id = current.current.roomId;
    const cursor = current.current.earlier;
    if (!id || !cursor) return;
    const token = ++msgToken.current;
    try {
      const page = await api.collaborationRoomMessages(id, null, cursor, PAGE);
      if (msgToken.current !== token) return;
      setMessages(mergeMessages(messagesRef.current, page.items).items);
      setEarlierCursor(page.next_cursor);
    } catch (e) {
      if (msgToken.current === token) setError(String(e));
    }
  }, []);

  useEffect(() => {
    let active = true;
    roomToken.current += 1;
    msgToken.current += 1;
    setLoading(true);
    setMessages([]);
    setEarlierCursor(null);
    setError("");
    void (async () => {
      const next = await refreshRoom();
      if (!active) return;
      const id = next?.room?.id;
      if (id) await loadLatest(id, true);
      if (active) setLoading(false);
    })();
    return () => { active = false; };
  }, [taskId, refreshRoom, loadLatest]);

  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    void listen<ChangedEvent>("collaboration-changed", event => {
      if (!active) return;
      const payload = event.payload ?? {};
      if (payload.task_id && payload.task_id !== taskId) return;
      if (payload.category === "member" || payload.category === "room") {
        void refreshRoom();
        return;
      }
      void refreshRoom().then(next => {
        const id = next?.room?.id;
        if (!id) return;
        if (current.current.roomId && current.current.roomId !== id) return;
        void loadLatest(id);
      });
    })
      .then(stop => { if (active) stops.push(stop); else stop(); })
      .catch(() => { /* 非 Tauri 环境（测试）没有事件通道 */ });
    return () => { active = false; stops.forEach(stop => stop()); };
  }, [taskId, refreshRoom, loadLatest]);

  return { room, messages, earlierCursor, loading, error, refreshRoom, loadLatest, loadEarlier, setRoom };
}
