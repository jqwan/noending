import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { copyToClipboard, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import type {
  CollaborationMemberState,
  CollaborationStartedMember,
  SessionListItem,
} from "../../types";
import CollaborationCandidates from "./CollaborationCandidates";
import CollaborationMembers from "./CollaborationMembers";
import RoomMessages from "./RoomMessages";
import { RUN_LABEL, joinPrompt } from "./collaborationState";
import { useCollaborationRoom } from "./useCollaborationRoom";
import "./collaboration.css";

function newRequestId(): string {
  const web = globalThis.crypto;
  if (web && typeof web.randomUUID === "function") return web.randomUUID();
  return `req-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** 旧或已停止的 run 统一报 run_inactive；要明说「已停止」，不能当成普通失败静默重试。 */
function sendErrorMessage(error: unknown): string {
  const text = String(error);
  if (/run_inactive/i.test(text)) return "本次协作已停止：消息被拒绝。停止后不会自动重开运行，请重新开启。";
  if (/stop|stale|过期/i.test(text)) return "本次协作已停止：发信被拒绝。停止后不会自动重启运行。";
  return `发送失败：${text}`;
}

/**
 * 任务详情／协作室。
 *
 * 一个任务最多一个协作室，一次开启一个 run。未开启时发送消息与开启是同一个
 * 后端事务；运行中掉线不自动结束任务、不替换负责人、不重启成员。
 */
export default function CollaborationRoomView({
  taskId,
  taskTitle,
  sessions,
  archived = false,
}: {
  taskId: string;
  taskTitle: string;
  sessions: SessionListItem[];
  archived?: boolean;
}) {
  const { room, messages, earlierCursor, loading, error, refreshRoom, loadLatest, loadEarlier, setRoom } =
    useCollaborationRoom(taskId);
  const [selection, setSelection] = useState<{ selected: CollaborationStartedMember[]; coordinator: string | null }>({
    selected: [],
    coordinator: null,
  });
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [waking, setWaking] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);

  const config = room?.room ?? null;
  const run = room?.run ?? null;
  const running = run?.status === "starting" || run?.status === "active";
  const title = room?.title || taskTitle;
  const enoughMembers = selection.selected.length >= 2;

  // 上次运行已停止时，沿用原配置重新开启，用户不必再勾一次。
  const restore = useMemo(() => {
    if (running || !config) return null;
    const selected = config.selected_members
      .map(session_id => {
        const previous = room?.members.find(m => m.session_id === session_id);
        return previous ? { session_id, wake_method: previous.wake_method } : null;
      })
      .filter((v): v is CollaborationStartedMember => v !== null);
    return selected.length ? { selected, coordinator: config.coordinator_session_id } : null;
  }, [running, config, room?.members]);

  useEffect(() => {
    if (restore && selection.selected.length === 0) setSelection(restore);
  }, [restore, selection.selected.length]);

  const start = async (initialMessage: string | null) => {
    if (busy || archived) return;
    const members = selection.selected.length ? selection.selected : restore?.selected ?? [];
    const coordinator = selection.coordinator ?? restore?.coordinator ?? null;
    if (members.length < 2) { setActionError("至少需要 2 个会话成员。"); return; }
    setBusy(true);
    setActionError("");
    try {
      const next = await api.collaborationRoomStart(
        taskId, members, config?.revision ?? 0, newRequestId(), coordinator, initialMessage,
      );
      setRoom(next);
      setDraft("");
      if (next.room?.id) await loadLatest(next.room.id, true);
    } catch (e) {
      setActionError(`开启失败：${String(e)}`);
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    if (busy || !config || !run) return;
    setBusy(true);
    setActionError("");
    try {
      setRoom(await api.collaborationRoomStop(config.id, run.id, config.revision, newRequestId()));
    } catch (e) {
      setActionError(`停止失败：${String(e)}`);
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const send = async () => {
    const body = draft.trim();
    if (!body || busy) return;
    if (!running || !config || !run) { await start(body); return; }
    setBusy(true);
    setActionError("");
    const submitted = draft;
    try {
      await api.collaborationRoomSend(config.id, run.id, body, newRequestId());
      setDraft(current => (current === submitted ? "" : current));
      await loadLatest(config.id);
    } catch (e) {
      setActionError(sendErrorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const wake = async (sessionId: string) => {
    if (!config || !run || waking) return;
    setWaking(sessionId);
    setActionError("");
    try {
      const member: CollaborationMemberState = await api.collaborationMemberWake(
        config.id, run.id, sessionId, newRequestId(),
      );
      setRoom(old => (old
        ? { ...old, members: old.members.map(m => (m.session_id === member.session_id ? member : m)) }
        : old));
    } catch (e) {
      setActionError(`打开失败：${String(e)}`);
    } finally {
      setWaking(null);
    }
  };

  const copyPrompt = async (sessionId: string) => {
    if (!config || !run) return;
    const text = joinPrompt({
      taskId,
      taskTitle: title,
      roomId: config.id,
      runId: run.id,
      selfSessionId: sessionId,
      coordinator: config.coordinator_session_id === sessionId,
    });
    showToast(await copyToClipboard(text) ? "已复制加入提示" : "复制失败");
  };

  if (loading) {
    return <section className="card detail-section" role="status"><p className="small muted">正在读取协作室…</p></section>;
  }

  return (
    <div className="detail-layout task-detail-layout">
      <div className="detail-main task-detail-main">
        <section className="card detail-section collab-room">
          <div className="row between" style={{ alignItems: "center" }}>
            <h2 className="detail-section-title">
              <Icon name="chatLines" />
              <span>协作室</span>
              {run && <span className={`badge small collab-run-${run.status}`}>{RUN_LABEL[run.status]}</span>}
            </h2>
            <button
              type="button"
              className={`btn small ${running ? "danger" : "primary"}`}
              disabled={busy || archived || (!running && !enoughMembers && !restore)}
              title={archived ? "已归档任务不能开启协作" : running ? "停止本次协作" : "开启协作"}
              onClick={() => void (running ? stop() : start(draft.trim() || null))}
            >
              {busy ? "处理中…" : running ? "停止协作" : "开启协作"}
            </button>
          </div>

          {archived && <p className="small muted" style={{ margin: 0 }}>任务已归档，不能开启新的协作；已有协作在归档时已被终止。</p>}
          {run?.status === "stopped" && (
            <p className="small muted" style={{ margin: 0 }}>
              上次协作已停止{run.stopped_at ? `（${timeAgo(run.stopped_at)}）` : ""}。消息保留可查阅；重新开启是新的运行。
            </p>
          )}
          {run?.status === "starting" && (
            <p className="small muted" style={{ margin: 0 }}>
              至少 2 名成员确认加入、且负责人加入后才进入协作中。单人失败可以单独重试。
            </p>
          )}
          {actionError && <p role="alert" className="small warning">{actionError}</p>}
          {error && <p role="alert" className="small warning">{error}</p>}

          {running ? (
            <CollaborationMembers
              room={{ ...room!, title }}
              sessions={sessions}
              now={now}
              busySessionId={waking}
              onWake={sessionId => void wake(sessionId)}
              onCopyPrompt={sessionId => void copyPrompt(sessionId)}
            />
          ) : (
            <CollaborationCandidates
              taskId={taskId}
              sessions={sessions}
              selected={selection.selected}
              coordinator={selection.coordinator}
              onChange={setSelection}
              disabled={busy || archived}
            />
          )}
        </section>

        <section className="card detail-section">
          <h2 className="detail-section-title">
            <Icon name="chat" />
            <span>协作消息{messages.length ? `（${messages.length}）` : ""}</span>
          </h2>
          <RoomMessages
            messages={messages}
            earlierCursor={earlierCursor}
            sessions={sessions}
            sending={busy}
            draft={draft}
            onDraftChange={setDraft}
            onSend={() => void send()}
            onLoadEarlier={() => void loadEarlier()}
            sendLabel={running ? "发送" : "发送并开启"}
            placeholder={running ? "在协作室说一句" : "首条消息（随开启一起写入）"}
            disabled={archived}
          />
        </section>
      </div>

      <aside className="detail-aside">
        <section className="card detail-section">
          <h2 className="detail-section-title">
            <Icon name="info" />
            <span>本次协作</span>
          </h2>
          <p className="small muted" style={{ margin: 0 }}>
            消息只在本房间保存一份。成员各自维护阅读位置；任务进展由负责人统一维护，成员在这里简要汇报。
          </p>
          {config && (
            <>
              <div className="small"><span className="muted">房间</span> <span className="mono">{config.id.slice(0, 8)}</span></div>
              <div className="small"><span className="muted">配置版本</span> {config.revision}</div>
            </>
          )}
          {run && (
            <div className="small">
              <span className="muted">运行</span> <span className="mono">{run.id.slice(0, 8)}</span>
              {" · "}{RUN_LABEL[run.status]}
              {run.started_at ? ` · 开始于 ${timeAgo(run.started_at)}` : ""}
            </div>
          )}
          {!config && <p className="small muted" style={{ margin: 0 }}>还没有协作室：勾选成员后开启会在保存配置的同时创建本次运行。</p>}
        </section>
      </aside>
    </div>
  );
}
