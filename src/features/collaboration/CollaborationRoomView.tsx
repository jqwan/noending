import { useEffect, useMemo, useRef, useState } from "react";
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

interface Selection {
  selected: CollaborationStartedMember[];
  coordinator: string | null;
}

/** 未确认的开启：请求可能已落库只是响应超时，重放时必须原参数、原 request_id。 */
interface UnconfirmedStart {
  requestId: string;
  revision: number;
  members: CollaborationStartedMember[];
  coordinator: string | null;
  initialMessage: string | null;
}

const EMPTY: Selection = { selected: [], coordinator: null };

function newRequestId(): string {
  const web = globalThis.crypto;
  if (web && typeof web.randomUUID === "function") return web.randomUUID();
  return `req-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

function sessionLabel(sessionId: string, sessions: SessionListItem[]): string {
  return sessions.find(s => s.id === sessionId)?.title || sessionId.slice(0, 8);
}

/** 旧或已停止的 run 统一报 run_inactive；要明说「已停止」，不能当成普通失败静默重试。 */
function sendErrorMessage(error: unknown): string {
  const text = String(error);
  if (/run_inactive/i.test(text)) return "本次协作已停止：消息被拒绝。停止后不会自动重开运行，请重新开启。";
  if (/stop|stale|过期/i.test(text)) return "本次协作已停止：发信被拒绝。停止后不会自动重启运行。";
  return `发送失败：${text}`;
}

function revisionErrorCopy(action: string, error: unknown): string {
  const text = String(error);
  if (/revision|conflict|冲突|已变化/i.test(text)) return `${action}失败：配置已变化，已重新读取，请再试一次。`;
  return `${action}失败：${text}`;
}

/**
 * 任务详情／协作室。
 *
 * 一个任务最多一个协作室，一次开启一个 run。未开启时发送消息与开启是同一个
 * 后端事务；运行中掉线不自动结束任务、不替换负责人、不重启成员。
 *
 * 写操作按「操作 + 参数」复用 request_id：同一请求传输失败后重试不会在后端
 * 留下第二份记录；参数变化才算新操作。未确认的开启单独保存原参数，
 * 即使 room 已刷新成运行中，重试也只重放那一次开启，不会把首条消息改成第二条。
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
  const [edited, setEdited] = useState<{ revision: number; value: Selection } | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [waking, setWaking] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const [now, setNow] = useState(() => Date.now());
  /** 未确认的开启：与别的写操作分开保存，不会被一次唤醒或停止覆盖。 */
  const [unconfirmed, setUnconfirmed] = useState<UnconfirmedStart | null>(null);
  /** 多槽：每个「操作 + 参数」各持一个 request_id，互不覆盖。 */
  const pending = useRef(new Map<string, string>());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 10_000);
    return () => clearInterval(timer);
  }, []);

  const config = room?.room ?? null;
  const run = room?.run ?? null;
  const running = run?.status === "starting" || run?.status === "active";
  const revision = config?.revision ?? 0;
  const title = room?.title || taskTitle;

  // 配置里保存的成员和唤醒方式：停止后再次开启沿用，用户也能在此基础上改。
  const saved = useMemo<Selection>(
    () => (config ? { selected: config.selected_members, coordinator: config.coordinator_session_id } : EMPTY),
    [config],
  );
  // 用户本次改动优先。空成员数组与 null 负责人都是有效选择，不能被配置回填覆盖。
  const selection = edited && edited.revision === revision ? edited.value : saved;
  const setSelection = (value: Selection) => setEdited({ revision, value });
  const enoughMembers = selection.selected.length >= 2;

  const requestId = (key: string) => {
    const existing = pending.current.get(key);
    if (existing) return existing;
    const id = newRequestId();
    pending.current.set(key, id);
    return id;
  };
  const settled = (key: string) => { pending.current.delete(key); };

  /** 提交/重放一次开启。重放时参数原样取自未确认记录，不受刷新后的配置影响。 */
  const submitStart = async (plan: UnconfirmedStart) => {
    setBusy(true);
    setActionError("");
    try {
      const next = await api.collaborationRoomStart(
        taskId, plan.members, plan.revision, plan.requestId, plan.coordinator, plan.initialMessage,
      );
      setUnconfirmed(null);
      setRoom(next);
      setEdited(null);
      // 只有还停在首条消息时才清空；这期间用户新写的内容要留着。
      setDraft(current => (current === (plan.initialMessage ?? "") ? "" : current));
      if (next.room?.id) await loadLatest(next.room.id, true);
    } catch (e) {
      setUnconfirmed(plan);
      setActionError(`开启未确认：${String(e)}。重试会沿用同一次请求，不会重复写入首条消息。`);
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const start = async (initialMessage: string | null) => {
    if (busy || archived) return;
    const members = selection.selected;
    const coordinator = selection.coordinator;
    if (members.length < 2) { setActionError("至少需要 2 个会话成员。"); return; }
    // 参数与未确认那一次完全一致才算重放；后端按 request_id 返回同一结果。
    const same = unconfirmed
      && unconfirmed.initialMessage === initialMessage
      && unconfirmed.coordinator === coordinator
      && JSON.stringify(unconfirmed.members) === JSON.stringify(members);
    await submitStart(same ? unconfirmed! : { requestId: newRequestId(), revision, members, coordinator, initialMessage });
  };

  const stop = async () => {
    if (busy || !config || !run) return;
    const key = `stop:${config.id}:${run.id}:${revision}`;
    setBusy(true);
    setActionError("");
    try {
      setRoom(await api.collaborationRoomStop(config.id, run.id, revision, requestId(key)));
      settled(key);
    } catch (e) {
      setActionError(revisionErrorCopy("停止", e));
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const changeCoordinator = async (sessionId: string | null) => {
    if (busy || !config || !run) return;
    const key = `coordinator:${config.id}:${run.id}:${revision}:${sessionId ?? ""}`;
    setBusy(true);
    setActionError("");
    try {
      setRoom(await api.collaborationRoomCoordinator(config.id, run.id, sessionId, revision, requestId(key)));
      settled(key);
    } catch (e) {
      setActionError(revisionErrorCopy("更换负责人", e));
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const send = async () => {
    const body = draft.trim();
    if (!body || busy) return;
    // 上次开启没拿到确认、房间却已刷新成运行中：先重放那一次开启。
    // 直接发 send 会把同一条首条消息变成第二条正文。
    if (unconfirmed) { await submitStart(unconfirmed); return; }
    if (!running || !config || !run) { await start(body); return; }
    const key = `send:${config.id}:${run.id}:${body}`;
    setBusy(true);
    setActionError("");
    const submitted = draft;
    try {
      await api.collaborationRoomSend(config.id, run.id, body, requestId(key));
      settled(key);
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
    const key = `wake:${config.id}:${run.id}:${sessionId}`;
    setWaking(sessionId);
    setActionError("");
    try {
      const member: CollaborationMemberState = await api.collaborationMemberWake(
        config.id, run.id, sessionId, requestId(key),
      );
      settled(key);
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
              disabled={busy || archived || (!running && !enoughMembers)}
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
          {unconfirmed && (
            <p className="small warning" role="status">
              上次开启未得到确认（后端可能已生效）。
              <button type="button" className="btn small" disabled={busy}
                onClick={() => void submitStart(unconfirmed)}>重试确认</button>
              会沿用同一次请求，不会重复写入首条消息。
            </p>
          )}
          {error && <p role="alert" className="small warning">{error}</p>}

          {running ? (
            <>
              <CollaborationMembers
                room={{ ...room!, title }}
                sessions={sessions}
                now={now}
                busySessionId={waking}
                disabled={busy}
                onWake={sessionId => void wake(sessionId)}
                onCopyPrompt={sessionId => void copyPrompt(sessionId)}
              />
              <fieldset className="collab-coordinator" disabled={busy}>
                <legend className="small">负责人（仅本次成员，可更换或清除）</legend>
                <label className="collab-radio">
                  <input type="radio" name="collab-run-coordinator" checked={!config?.coordinator_session_id}
                    onChange={() => void changeCoordinator(null)} />
                  <span>不由成员负责，由我协调</span>
                </label>
                {room!.members.map(member => (
                  <label key={member.session_id} className="collab-radio">
                    <input type="radio" name="collab-run-coordinator"
                      checked={config?.coordinator_session_id === member.session_id}
                      onChange={() => void changeCoordinator(member.session_id)} />
                    <span className="truncate">{sessionLabel(member.session_id, sessions)}</span>
                  </label>
                ))}
              </fieldset>
            </>
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
            <div className="small"><span className="muted">房间</span> <span className="mono">{config.id.slice(0, 8)}</span></div>
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
