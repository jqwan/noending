import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { CollaborationMessage, CollaborationRoomView } from "../../types";

/** 后端分页上限是 100；历史一次取满，界面不再做分页按钮。 */
const PAGE = 100;

/** 合并同一房间的追加消息：保留已有历史，按 ID 去重、seq 升序。 */
export function mergeMessages(
  old: CollaborationMessage[],
  page: CollaborationMessage[],
): CollaborationMessage[] {
  const byId = new Map(old.map(m => [m.id, m]));
  for (const m of page) byId.set(m.id, m);
  return [...byId.values()].sort((a, b) => a.seq - b.seq);
}

interface MessageRead {
  roomId: string;
  generation: number;
  pending: boolean;
  full: boolean;
  promise: Promise<void>;
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
 * 首屏取完整历史，后续向前补读到已保存的末尾；同一房间合并刷新请求，
 * member / room 类变更只重读房间，不动消息列表。
 */
export function useCollaborationRoom(taskId: string) {
  const [room, setRoom] = useState<CollaborationRoomView | null>(null);
  const [messages, setMessages] = useState<CollaborationMessage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const roomToken = useRef(0);
  const generation = useRef(0);
  const messageRead = useRef<MessageRead | null>(null);
  const messageOwner = useRef<string | null>(null);
  const messagesRef = useRef<CollaborationMessage[]>([]);
  const roomId = room?.room?.id ?? null;
  const current = useRef<string | null>(null);
  current.current = roomId;

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

  /** 同一房间只跑一个读取流程；途中的通知合并成下一轮，不取消正在补读的历史。 */
  const readMessages = useCallback((id: string, full: boolean): Promise<void> => {
    const existing = messageRead.current;
    if (existing?.roomId === id && existing.generation === generation.current) {
      existing.pending = true;
      existing.full ||= full;
      return existing.promise;
    }
    if (messageOwner.current !== id) {
      messageOwner.current = id;
      messagesRef.current = [];
      setMessages([]);
    }
    const read: MessageRead = {
      roomId: id, generation: generation.current, pending: false, full,
      promise: Promise.resolve(),
    };
    messageRead.current = read;
    const currentRead = () => messageRead.current === read && generation.current === read.generation;
    read.promise = (async () => {
      try {
        do {
          read.pending = false;
          const knownThrough = read.full ? null : messagesRef.current[messagesRef.current.length - 1]?.seq ?? null;
          read.full = false;
          const collected: CollaborationMessage[] = [];
          const cursors = new Set<string>();
          let cursor: string | null = null;
          while (true) {
            const page = await api.collaborationRoomMessages(id, null, cursor, PAGE);
            if (!currentRead()) return;
            collected.push(...page.items);
            const first = page.items[0]?.seq;
            if (!page.next_cursor || (knownThrough !== null && first !== undefined && first <= knownThrough + 1)) break;
            if (cursors.has(page.next_cursor)) throw new Error("协作消息分页游标未推进，请重试。");
            cursors.add(page.next_cursor);
            cursor = page.next_cursor;
          }
          messagesRef.current = mergeMessages(messagesRef.current, collected);
          setMessages(messagesRef.current);
          setError("");
          setLoading(false);
        } while (read.pending && currentRead());
      } catch (e) {
        if (currentRead()) setError(String(e));
      } finally {
        if (messageRead.current === read) messageRead.current = null;
      }
    })();
    return read.promise;
  }, []);

  const loadAll = useCallback((id: string) => readMessages(id, true), [readMessages]);
  const loadLatest = useCallback((id: string) => readMessages(id, false), [readMessages]);

  useEffect(() => {
    let active = true;
    roomToken.current += 1;
    generation.current += 1;
    messageRead.current = null;
    messageOwner.current = null;
    messagesRef.current = [];
    setLoading(true);
    setMessages([]);
    setError("");
    void (async () => {
      const next = await refreshRoom();
      if (!active) return;
      const id = next?.room?.id;
      if (id) await loadAll(id);
      if (active) setLoading(false);
    })();
    return () => {
      active = false;
      roomToken.current += 1;
      generation.current += 1;
      messageRead.current = null;
    };
  }, [taskId, refreshRoom, loadAll]);

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
        if (!active || !id) return;
        if (current.current && current.current !== id) return;
        void loadLatest(id);
      });
    })
      .then(stop => { if (active) stops.push(stop); else stop(); })
      .catch(() => { /* 非 Tauri 环境（测试）没有事件通道 */ });
    return () => { active = false; stops.forEach(stop => stop()); };
  }, [taskId, refreshRoom, loadLatest]);

  return { room, messages, loading, error, refreshRoom, loadAll, loadLatest, setRoom };
}
