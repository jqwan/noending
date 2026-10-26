import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { CollaborationMessage, CollaborationRoomView } from "../../types";

const PAGE = 30;

/**
 * 合并同一房间的消息：按 seq 去重升序。
 * 新一页与已有序列之间出现空档（比如一次来了超过一页）时，旧页不再可信，
 * 直接以新页为准，避免留下永远补不上的洞。
 */
export function mergeMessages(old: CollaborationMessage[], page: CollaborationMessage[]): CollaborationMessage[] {
  if (!page.length) return old;
  const maxOld = old.length ? old[old.length - 1].seq : 0;
  const minNew = page[0].seq;
  if (old.length && minNew > maxOld + 1) return page;
  const byId = new Map(old.map(m => [m.id, m]));
  for (const m of page) byId.set(m.id, m);
  return [...byId.values()].sort((a, b) => a.seq - b.seq);
}

interface ChangedEvent {
  task_id?: string;
  room_id?: string;
  run_id?: string;
  /** member｜heartbeat｜ack｜wake｜room｜message｜run；两版契约命名并存，都按语义归类。 */
  category?: string;
}

/**
 * 只改成员状态的变更：心跳续租、阅读位置、唤醒结果、配置变更。
 * 这些不产生消息，重载历史既贵又会让「心跳不刷全部消息」的约束失效。
 */
const MEMBER_ONLY = new Set(["member", "heartbeat", "ack", "wake", "room"]);

/**
 * 协作室读取：房间状态与消息分开刷新。
 *
 * `collaboration-changed` 的 member 类变更（心跳、在线状态）只重读房间，
 * 不动消息列表；其余类别才追加消息。opaque cursor 由后端给出方向，
 * 前端只回传，不按时间戳或条数推算。
 */
export function useCollaborationRoom(taskId: string) {
  const [room, setRoom] = useState<CollaborationRoomView | null>(null);
  const [messages, setMessages] = useState<CollaborationMessage[]>([]);
  const [earlierCursor, setEarlierCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const roomToken = useRef(0);
  const msgToken = useRef(0);
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
      setMessages(old => (reset ? page.items : mergeMessages(old, page.items)));
      if (reset) setEarlierCursor(page.next_cursor);
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
      setMessages(old => mergeMessages(old, page.items));
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
      if (payload.category && MEMBER_ONLY.has(payload.category)) {
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
