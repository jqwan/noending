import { DetailLayout } from "../../components/DetailLayout";
import { DetailFields, DetailLoading, DetailSection } from "../../components/DetailSection";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api } from "../../api";
import { timeAgo, Field } from "../../components/common";
import type {
  CollaborationMemberState,
  CollaborationStartedMember,
  WorkstreamProject,
} from "../../types";
import CollaborationSetupModal, { type CollaborationSelection } from "./CollaborationSetupModal";
import CollaborationCoordinator from "./CollaborationCoordinator";
import CollaborationMembers from "./CollaborationMembers";
import RoomMessages from "./RoomMessages";
import { RUN_LABEL } from "./collaborationState";
import { useCollaborationRoom } from "./useCollaborationRoom";
import "./collaboration.css";
import type { Route } from "../../app/routes";

/** 未确认的开启：请求可能已落库只是响应超时，重放时必须原参数、原 request_id。 */
interface UnconfirmedStart {
  requestId: string;
  revision: number;
  members: CollaborationStartedMember[];
  coordinator: string | null;
  initialMessage: string | null;
}

const EMPTY: CollaborationSelection = { selected: [], coordinator: null };

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
  projects,
  archived = false,
  navigate,
  showChat = true,
  children,
  detailsPanel,
}: {
  taskId: string;
  taskTitle: string;
  projects: WorkstreamProject[];
  archived?: boolean;
  navigate: (route: Route) => void;
  showChat?: boolean;
  children?: ReactNode;
  detailsPanel?: ReactNode;
}) {
  const { room, roomLoading, messages, loading, error, refreshRoom, loadAll, loadLatest, setRoom } =
    useCollaborationRoom(taskId, showChat);
  const [edited, setEdited] = useState<{ revision: number; value: CollaborationSelection } | null>(null);
  const [setupOpen, setSetupOpen] = useState(false);
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
  const saved = useMemo<CollaborationSelection>(() => {
    if (!config || !room) return EMPTY;
    return {
      selected: room.members.map(({ session_id, wake_method, title, project_id }) => ({ session_id, wake_method, title, project_id })),
      coordinator: config.coordinator_session_id,
    };
  }, [config, room]);
  const draftSelection = edited && edited.revision === revision ? edited.value : saved;
  const selected = draftSelection.selected.filter(member => projects.some(project => project.id === member.project_id));
  const selection: CollaborationSelection = {
    selected,
    coordinator: selected.some(member => member.session_id === draftSelection.coordinator) ? draftSelection.coordinator : null,
  };
  const setSelection = (value: CollaborationSelection) => setEdited({ revision, value });
  const openSetup = () => {
    if (roomLoading || busy || archived) return;
    if (!unconfirmed) setEdited(null);
    setSetupOpen(true);
  };
  useEffect(() => {
    if (running || archived) setSetupOpen(false);
  }, [running, archived]);

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
    setSetupOpen(false);
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
      setDraft(current => (current.trim() === (plan.initialMessage ?? "") ? "" : current));
      if (next.room?.id) await loadAll(next.room.id);
    } catch (e) {
      setUnconfirmed(plan);
      setActionError(`开启未确认：${String(e)}。重试会沿用同一次请求，不会重复写入首条消息。`);
      void refreshRoom();
    } finally {
      setBusy(false);
    }
  };

  const start = async (initialMessage: string | null) => {
    if (busy || archived || running) return;
    const members = selection.selected.map(({ session_id, wake_method }) => ({ session_id, wake_method }));
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
    if (!running || !config || !run) { openSetup(); return; }
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

  const feedback = ((actionError || unconfirmed || error) && <div className="collab-chat-feedback">
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
        </div>);

  return (<>
    <DetailLayout mainClassName={showChat ? "detail-main-collaboration" : ""} label="任务详情栏" panel={<><DetailSection sectionId="collaboration-members" label="协作成员" title="协作成员">
          {roomLoading ? <DetailLoading>正在读取协作成员…</DetailLoading> : running ? (
            <CollaborationMembers
              room={{ ...room!, title }}
              now={now}
              busySessionId={waking}
              disabled={busy}
              onWake={sessionId => void wake(sessionId)}
              navigate={navigate}
            />
          ) : null}
        <div className="collab-run-controls" role="group" aria-label="本次协作">
          {running && <CollaborationCoordinator
            value={config?.coordinator_session_id ?? null}
            disabled={busy || archived}
            members={room!.members.map(member => ({ id: member.session_id, title: member.title || member.session_id.slice(0, 8) }))}
            onChange={id => void changeCoordinator(id)} />}
          <div className="collab-run-toolbar">
            <div className="collab-run-status small muted">
              <span className={`badge small${run ? ` collab-run-${run.status}` : ""}`}>{archived ? "已归档" : run ? RUN_LABEL[run.status] : "未开启"}</span>
              {running && <span className="collab-run-count">{room!.members.length} 名成员</span>}
            </div>
            <button type="button" className={`btn small ${running ? "danger" : "primary"}`}
              disabled={roomLoading || busy || archived}
              title={archived ? "已归档任务不能开启协作" : undefined}
              onClick={() => { if (running) void stop(); else openSetup(); }}>
              {busy ? "处理中…" : running ? "停止协作" : "选择成员"}
            </button>
          </div>
          {run?.status === "starting" && <p className="small muted">等待至少 2 名成员及负责人加入。</p>}
          {(config || run) && <details className="collab-run-details small muted">
            <summary>协作信息</summary>
            <DetailFields>
              {config && <Field label="房间 ID" copyValue={config.id}><span className="mono">{config.id}</span></Field>}
              {run && <Field label="运行 ID" copyValue={run.id}><span className="mono">{run.id}</span></Field>}
              {run?.started_at && <Field label="开始时间">{timeAgo(run.started_at)}</Field>}
              {run?.stopped_at && <Field label="停止时间">{timeAgo(run.stopped_at)}</Field>}
            </DetailFields>
          </details>}
        </div>
        </DetailSection>
        {detailsPanel}</>}>{showChat ? <section className="collab-chat" aria-label="协作室聊天">
        {feedback}
        <RoomMessages
          key={taskId}
          messages={messages}
          sending={busy}
          draft={draft}
          onDraftChange={setDraft}
          onSend={() => void send()}
          onSetup={!running && !archived ? openSetup : undefined}
          sendLabel={running ? "发送" : "选择成员并发送"}
          placeholder={running ? "在协作室说一句" : "首条消息（随开启一起写入）"}
          disabled={archived || loading}
          loading={loading}
        />
      </section> : <>{feedback}{children}</>}</DetailLayout>
    {setupOpen && !running && !archived && <CollaborationSetupModal
      taskId={taskId} projects={projects} selection={selection} firstMessage={draft.trim() || null}
      navigate={navigate} onChange={setSelection} onClose={() => setSetupOpen(false)}
      onStart={() => void start(draft.trim() || null)} />}
  </>);
}
