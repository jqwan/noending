import SummaryMarkdown from "./SummaryMarkdown";
import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { contextUpdateErrorCopyText, contextUpdateErrorDetails, copyToClipboard, openPath, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { messageData, type SessionMessageData } from "./SessionMessage";
import SessionSubpageTabs from "./SessionSubpageTabs";
import SessionHeaderActions from "./SessionHeaderActions";
import PermanentDeleteModal from "./PermanentDeleteModal";
import SessionTaskTrackingModal from "./SessionTaskTrackingModal";
import {
  agentDisplayLabel,
  formatDateTime,
  projectCellFor,
  sessionDisplayTitle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import {
  type Agent,
  type Project,
  type SessionContextView,
  type SessionDetail,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Session Detail：一个逻辑会话（用户可感知、可 Resume 的主会话）的四块内容——
 * Conversation（只有 user/assistant prose）、Execution Info、Source / Lifecycle、Context / task tracking。
 * 唯一的会话→会话链接是 fork 来源。
 */
export const sessionDetailCache = new Map<string, SessionDetail>();

export default function SessionDetailView({
  sessionId,
  initialTitle,
  initialAgent,
  navigate,
  goBack,
}: {
  sessionId: string;
  initialTitle?: string;
  initialAgent?: Agent;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(() => sessionDetailCache.get(sessionId) ?? null);
  const [failed, setFailed] = useState(false);
  /** Context：纯读取的Markdown 摘要 + 显式「更新摘要 / 重新生成」。 */
  const [sessionCtx, setSessionCtx] = useState<SessionContextView | null>(null);
  const [ctxBusy, setCtxBusy] = useState(false);
  const [ctxReadError, setCtxReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [preferenceError, setPreferenceError] = useState<string | null>(null);
  const [ctxError, setCtxError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const currentSessionId = useRef(sessionId);
  // 已归档动作（Session Lifecycle & Deletion）：执行中的 busy、
  // 以及从详情页直接发起的删除 Modal。归档的确认与执行在
  // SessionHeaderActions（三个子页共用）。
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);
  const [trackModalOpen, setTrackModalOpen] = useState(false);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字；
   * 派生链自带名字，正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  useEffect(() => {
    setCtxError(null);
    setPreferenceError(null);
    setCtxReadError(null);
    setCtxBusy(false);
    if (currentSessionId.current !== sessionId) {
      currentSessionId.current = sessionId;
      setDetail(sessionDetailCache.get(sessionId) ?? null);
    }
  }, [sessionId]);

  /** 打开 / 刷新这一页：详情与 Session Context 都是纯读取。 */
  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => {
        sessionDetailCache.set(sessionId, d);
        setDetail(d);
        setFailed(false);
      })
      .catch((e) => { console.error(e); setFailed(true); });
    api.getSessionContext(sessionId)
      .then((c) => { setSessionCtx(c); setCtxReadError(null); })
      .catch((e) => { console.error(e); setCtxReadError(contextUpdateErrorDetails(e)); });
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const needsProjectName = !detail?.workspace_path && !!detail?.session.project_id;
  useEffect(() => {
    if (!needsProjectName || projects) return;
    api.listProjects().then(setProjects).catch(console.error);
  }, [needsProjectName, projects]);
  const projectNameById = useMemo(
    () => new Map((projects ?? []).map((p) => [p.id, p.name])),
    [projects],
  );

  const fallbackAgent = initialAgent;
  const fallbackRawTitle = initialTitle;
  const fallbackDisplayTitle = fallbackRawTitle ? sessionDisplayTitle(fallbackRawTitle) : "";

  const fallbackHeaderTitle = fallbackAgent ? (
    <span className="session-title-with-icon" title={typeof fallbackDisplayTitle === "string" ? fallbackDisplayTitle : undefined}>
      <AgentIcon agent={fallbackAgent} size={18} />
      <span className="session-title-text">{fallbackDisplayTitle}</span>
    </span>
  ) : (
    fallbackDisplayTitle
  );

  /**
   * 没拿到数据时也要把 PageHeader 画出来：标题行吸在应用标题栏那条 band 上，
   * 加载态若不渲染它，整条标题栏会先消失再补回来，比"内容区里一行加载中"显眼得多。
   */
  if (!detail) {
    return (
      <div className="main session-detail" role="status">
        <PageHeader title={fallbackHeaderTitle} actions={<SessionSubpageTabs sessionId={sessionId} title={fallbackDisplayTitle} agent={fallbackAgent} navigate={navigate} />}>
          {failed && (
            <>
              <p className="muted small">
                这个 Session 可能已经被 Agent 自己清理。本地数据没有被修改，可以重试。
              </p>
              <div className="invite">
                <button className="btn small" onClick={refresh}>重试</button>
              </div>
            </>
          )}
        </PageHeader>
        {!failed && (
          <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
            <div className="skeleton" style={{ height: 120, borderRadius: "var(--radius-lg)" }} />
            <div className="skeleton" style={{ height: 180, borderRadius: "var(--radius-lg)" }} />
          </div>
        )}
      </div>
    );
  }
  const { session } = detail;

  /** 一次点击 → 最多启动一次 Agent → 一份新的 Markdown 摘要。失败按后端原因给可行动文案。 */
  const updateSummary = async (regenerate = false) => {
    if (ctxBusy) return;
    const updateSessionId = sessionId;
    setCtxBusy(true);
    setCtxReadError(null);
    setCtxError(null);
    setPreferenceError(null);
    try {
      const out = regenerate
        ? await api.regenerateSessionContext(updateSessionId)
        : await api.updateSessionContext(updateSessionId);
      if (currentSessionId.current !== updateSessionId) return;
      setPreferenceError(out.preference_error);
      showToast(
        out.status === "no_change"
          ? "没有新内容，摘要保持不变"
          : out.status === "skipped"
            ? "没有新的摘要内容"
            : out.status === "partial"
            ? "已更新摘要（还有内容待下次更新）"
            : regenerate ? "已重新生成摘要" : "已更新摘要",
      );
      refresh();
    } catch (e) {
      console.error(e);
      if (currentSessionId.current === updateSessionId) {
        setCtxError(contextUpdateErrorDetails(e));
      }
    } finally {
      if (currentSessionId.current === updateSessionId) setCtxBusy(false);
    }
  };

  /** 复制当前摘要：保留 Markdown 原文。 */
  const copyContext = async () => {
    if (!sessionCtx?.fields) return;
    const ok = await copyToClipboard(sessionCtx.fields.summary);
    showToast(ok ? "已复制会话摘要" : "复制失败");
  };

  const title = sessionDisplayTitle(session.title);
  const untitled = title === UNTITLED_SESSION;
  const cwd = (session.cwd ?? "").trim();
  /** 单一生命周期权威：null = 正常，时间戳 = 在已归档。 */
  const archived = session.archived_at !== null;

  /** 从已归档恢复：archived_at 清空后本页就地回到正常形态。 */
  const doRestore = async () => {
    if (archiveBusy) return;
    setArchiveBusy(true);
    try {
      await api.restoreSession(sessionId);
      showToast("已恢复");
      refresh();
    } catch (e) {
      console.error(e);
      showToast(String(e));
    } finally {
      setArchiveBusy(false);
    }
  };
  /** 派生链自带的路径与 Project。 */
  const workspacePath = detail.workspace_path;
  const projectIdOnlyCell = projectCellFor(session, projectNameById);
  const derivedProjectName = workspacePath && workspacePath.project_name.trim() !== ""
    ? workspacePath.project_name
    : null;
  /** 这条会话「属于」哪个项目：工作目录与 Session API 使用相同的项目派生链。 */

  /** Conversation：只有 root 的 user/assistant prose。仅展示最近 3 条消息。 */
  const messages: SessionMessageData[] = detail.messages.map((m) => messageData(m, session.agent));
  const recentMessages = messages.slice(-3);
  const hasMessages = messages.length > 0 || detail.ingested_message_sequence > 0;

  /** 更新处理增量；重新生成即使没有增量也可从完整历史重建。归档不影响这两项操作。 */
  const canUpdateSummary =
    sessionCtx !== null
    && hasMessages
    && sessionCtx.pending;
  const canRegenerateSummary = sessionCtx !== null && hasMessages;

  /** 源会话：会话自己的源文件 + 详情加载时的新鲜结论。 */
  const sourceMissing = detail.source_status === "missing";
  const sourceUnavailable = detail.source_status === "unavailable";
  /** 源在，路径本身才是入口；源不在就只是可复制的文本，点了也只会报错。 */
  const canRevealSource = !sourceMissing && !sourceUnavailable;

  const revealSource = async () => {
    try {
      await api.revealSessionSource(sessionId);
    } catch (e) {
      showToast(`定位失败：${String(e)}`);
    }
  };

  return (
    <div className="main session-detail">
      <PageHeader
        title={
          <span className="session-title-with-icon" title={title}>
            <AgentIcon agent={session.agent} size={18} />
            <span className="session-title-text">{title}</span>
          </span>
        }
        actions={(
          <>
            {/* 顶部切换：历史消息、详情和终端实例。 */}
            <SessionSubpageTabs
              sessionId={sessionId}
              title={title}
              entry={undefined}
              agent={session.agent}
              sourceKind={session.source_kind}
              archived={archived}
              navigate={navigate}
            />
            <SessionHeaderActions
              sessionId={sessionId}
              agent={session.agent}
              sourceKind={session.source_kind}
              title={title}
              archived={archived}
              cwdExists={workspacePath ? workspacePath.exists : (detail.session.cwd ? detail.can_resume : true)}
              onChanged={refresh}
              onPurge={() => setPurgeOpen(true)}
              onRestore={doRestore}
            />
          </>
        )}
      />

      {/* 右栏承载只读事实（Agent、条数、会话信息），左栏是内容本身（摘要）。 */}
      <div className="detail-layout session-detail-layout">
        <div className="detail-main session-detail-main">
          {/* 当前摘要只读；更新合并增量，重新生成从完整历史重建。 */}
          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <div className="detail-section-title">
                <Icon name="spark" />
                <span>会话摘要</span>
              </div>
              <div className="row" style={{ gap: 8 }}>
                <button
                  className="btn small ghost"
                  disabled={!sessionCtx?.fields}
                  title={sessionCtx?.fields ? "复制摘要" : "暂无摘要"}
                  onClick={copyContext}
                >
                  复制
                </button>
                {canRegenerateSummary && (
                  <button className="btn small ghost" disabled={ctxBusy} onClick={() => updateSummary(true)}>
                    重新生成
                  </button>
                )}
                {canUpdateSummary && (
                  <button className="btn small primary" disabled={ctxBusy} onClick={() => updateSummary()}>
                    {ctxBusy ? "更新中…" : "更新摘要"}
                  </button>
                )}
              </div>
            </div>
            {sessionCtx?.pending && (
              <div className="small muted">
                有新消息待并入摘要。
              </div>
            )}
            {ctxReadError && (
              <div className="badge warn" style={{ overflowWrap: "anywhere" }}>
                {ctxReadError.message}
              </div>
            )}
            {preferenceError && <p role="alert" className="danger">摘要已处理，偏好未保存：{preferenceError}</p>}
            {ctxError && (
              <div className="badge warn" style={{ overflowWrap: "anywhere", display: "flex", gap: 8, alignItems: "center" }}>
                <span>{ctxError.message}{ctxError.operationId ? ` · 操作 ID ${ctxError.operationId}` : ""}</span>
                {ctxError.operationId && (
                  <button className="btn small ghost" onClick={async () => {
                    const ok = await copyToClipboard(contextUpdateErrorCopyText(ctxError));
                    showToast(ok ? "已复制错误详情" : "复制失败");
                  }}>复制错误详情</button>
                )}
              </div>
            )}
            {sessionCtx?.fields ? (
              <div className="md-body" style={{ overflowWrap: "anywhere" }}>
                <SummaryMarkdown>
                  {sessionCtx.fields.summary}
                </SummaryMarkdown>
              </div>
            ) : (
              <div className="l1-none">
                {!hasMessages ? "暂无消息" : "暂无摘要"}
              </div>
            )}
          </section>

          {/* 消息 */}
          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <div className="detail-section-title">
                <Icon name="chatLines" />
                <span>最近消息</span>
              </div>
              {detail.ingested_message_sequence > 0 && (
                <button
                  className="btn small ghost"
                  onClick={() =>
                    navigate({
                      view: "session",
                      sessionId,
                      entry: "conversation",
                      initialTitle: title,
                      initialAgent: session.agent,
                      initialTotal: detail.ingested_message_sequence,
                    })
                  }
                >
                  查看全部历史消息（共 {detail.ingested_message_sequence} 条）
                </button>
              )}
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {recentMessages.map((m) => <SessionMessage key={m.sequence} msg={m} />)}
              {messages.length === 0 && (
                <div className="empty">
                  还没有同步消息。
                  <div className="small" style={{ marginTop: 4 }}>
                    重新读取以加载已同步的原始会话。
                  </div>
                  <div className="invite">
                    <button className="btn small" onClick={refresh}>重新读取</button>
                  </div>
                </div>
              )}
            </div>
          </section>
        </div>

        <aside className="detail-aside session-detail-aside">
          {/* 只读事实在右栏模块化卡片中展示 */}
          <section className="card detail-section">
            <div className="detail-section-title">
              <Icon name="info" />
              <span>会话信息</span>
            </div>
            {(untitled || archived) && (
              <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
                {archived && <span className="badge warn">已归档</span>}
              </div>
            )}

            {/* 执行事实：一次会话「在什么时候、哪个目录、源在哪里」。值可整段选中并复制。 */}
            <div className="session-info-fields">
              <Field label="Agent">
                <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                  <button
                    type="button"
                    className="link"
                    title={`打开 Agent：${agentDisplayLabel(session.agent)}`}
                    style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
                    onClick={() => navigate({ view: "agents", agent: session.agent })}
                  >
                    <AgentIcon agent={session.agent} size={16} />
                    <span>{agentDisplayLabel(session.agent)}</span>
                  </button>
                </span>
              </Field>
              <Field label="项目">
                {workspacePath ? (
                  derivedProjectName ? (
                    <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                      <button
                        className="link"
                        title={`打开项目：${derivedProjectName}`}
                        style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
                        onClick={() => navigate({ view: "project", projectId: workspacePath.project_id, initialTitle: derivedProjectName })}
                      >
                        <Icon name="project" />
                        <span>{derivedProjectName}</span>
                      </button>
                    </span>
                  ) : (
                    <span className="muted small">项目信息暂不可用</span>
                  )
                ) : session.project_id ? (
                  <span className="muted small" style={{ wordBreak: "break-word" }}>
                    {projectIdOnlyCell.text}
                    {" —— "}
                    {projectIdOnlyCell.hint}
                  </span>
                ) : (
                  <span className="muted small">无关联项目</span>
                )}
              </Field>
              <Field
                label="工作目录"
                copyValue={workspacePath ? workspacePath.canonical_path : (cwd || undefined)}
                copyTitle="复制工作目录"
              >
                {workspacePath ? (
                  <>
                    <button
                      type="button"
                      className="link mono path-text-with-icon"
                      title={workspacePath.canonical_path}
                      onClick={() => void openPath(workspacePath.canonical_path)}
                    >
                      <Icon name="folder" />
                      <span>{workspacePath.canonical_path}</span>
                    </button>
                    {cwd !== "" && cwd !== workspacePath.canonical_path && (
                      <div className="muted small" style={{ marginTop: 4 }}>
                        Agent 原始记录：
                        <button
                          type="button"
                          className="link mono path-text-with-icon"
                          title={cwd}
                          onClick={() => void openPath(cwd)}
                        >
                          <Icon name="folder" />
                          <span>{cwd}</span>
                        </button>
                      </div>
                    )}
                    {!workspacePath.exists && (
                      <div style={{ marginTop: 4 }}>
                        <span
                          className="badge warn"
                          title="磁盘中未找到该目录"
                        >
                          目录不存在
                        </span>
                      </div>
                    )}
                  </>
                ) : cwd ? (
                  <>
                    <button
                      type="button"
                      className="link mono path-text-with-icon"
                      title={cwd}
                      onClick={() => void openPath(cwd)}
                    >
                      <Icon name="folder" />
                      <span>{cwd}</span>
                    </button>
                    <div className="muted small" style={{ marginTop: 4 }}>
                      未登记为工作路径
                    </div>
                  </>
                ) : (
                  <span className="muted">{NO_CWD}（无目录记录）</span>
                )}
              </Field>
              <Field label="开始时间">
                {session.started_at
                  ? <span title={session.started_at}>{formatDateTime(session.started_at)}</span>
                  : <span className="muted">未知</span>}
              </Field>
              <Field label="最近活动">
                {session.last_activity_at ? (
                  <span title={session.last_activity_at}>
                    {timeAgo(session.last_activity_at)}
                    <span className="muted small"> · {formatDateTime(session.last_activity_at)}</span>
                  </span>
                ) : <span className="muted">未知</span>}
              </Field>
              {archived && session.archived_at && (
                <Field label="归档时间">
                  <span title={session.archived_at}>{formatDateTime(session.archived_at)}</span>
                </Field>
              )}
              <Field label="会话统计">
                <span className="row" style={{ gap: 12, flexWrap: "wrap" }}>
                  <span>用户消息 {detail.message_stats?.user_messages ?? detail.messages.filter((m) => m.role === "user").length}</span>
                  <span>Agent 回复 {detail.message_stats?.assistant_messages ?? detail.messages.filter((m) => m.role === "assistant").length}</span>
                </span>
              </Field>
              {/* 源会话：会话自己的源文件。
                  状态是详情加载时对源的新鲜结论，missing / unavailable 都如实说出。
                  路径本身就是「在文件管理器中显示」的入口，不再另配一行链接。 */}
              <Field
                label="源会话"
                copyValue={session.source_path}
                copyTitle="复制源会话路径"
              >
                {canRevealSource ? (
                  <button
                    className="link mono"
                    title={session.source_path}
                    style={{ minWidth: 0, wordBreak: "break-all", textAlign: "left" }}
                    onClick={() => void revealSource()}
                  >
                    {session.source_path}
                  </button>
                ) : (
                  <span
                    className="mono"
                    title={session.source_path}
                    style={{ minWidth: 0, wordBreak: "break-all", userSelect: "all" }}
                  >
                    {session.source_path}
                  </span>
                )}
                {sourceMissing && (
                  <div className="session-source-warning">源会话已不存在</div>
                )}
                {sourceUnavailable && (
                  <div className="session-source-warning">无法确认源会话状态</div>
                )}
              </Field>
              <Field label="会话 ID" copyValue={session.id} copyTitle="复制会话 ID">
                <div className="mono" style={{ wordBreak: "break-all", userSelect: "all" }}>
                  {session.id}
                </div>
              </Field>
              <Field
                label="Agent 会话 ID"
                copyValue={session.root_agent_session_id}
                copyTitle="复制 Agent 会话 ID"
              >
                <div
                  className="mono"
                  title="原始会话 ID"
                  style={{ wordBreak: "break-all", userSelect: "all" }}
                >
                  {session.root_agent_session_id}
                </div>
              </Field>
              {/* Fork：唯一的会话→会话链接。来源只是 provenance，
                  生命周期完全独立；来源不在本地库时也如实说明。 */}
              {(detail.forked_from || session.forked_from_session_id) && (
                <Field label="来源">
                  {detail.forked_from ? (
                    <button
                      className="link"
                      style={{ textAlign: "left", wordBreak: "break-word" }}
                      title={`打开来源会话：${sessionDisplayTitle(detail.forked_from.title)}`}
                      onClick={() => navigate({ view: "session", sessionId: detail.forked_from!.id, initialTitle: sessionDisplayTitle(detail.forked_from!.title), initialAgent: detail.forked_from!.agent })}
                    >
                      分叉自：{sessionDisplayTitle(detail.forked_from.title)}
                      {detail.forked_from.archived_at !== null && (
                        <span className="badge warn" style={{ marginLeft: 6 }}>已归档</span>
                      )}
                    </button>
                  ) : (
                    <span className="muted small">
                      来源会话不在 NoEnding 库里
                      <span className="mono" style={{ wordBreak: "break-all" }}>
                        {" · "}{session.forked_from_session_id}
                      </span>
                    </span>
                  )}
                </Field>
              )}
            </div>
          </section>

          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <div className="detail-section-title">
                <Icon name="tasks" />
                <span>跟踪任务 ({detail.tracked_workstreams.length})</span>
              </div>
              <button
                type="button"
                className="btn ghost icon-button small"
                title="添加跟踪任务"
                aria-label="添加跟踪任务"
                onClick={() => setTrackModalOpen(true)}
              >
                <Icon name="plus" />
              </button>
            </div>

            {detail.tracked_workstreams.length === 0 ? (
              <div className="l1-none">未跟踪任何任务</div>
            ) : (
              <div className="session-tracked-list">
                {detail.tracked_workstreams.map((task) => (
                  <div
                    key={task.id}
                    className="list-row session-tracked-task-row"
                    role="link"
                    tabIndex={0}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title });
                      }
                    }}
                    onClick={() => navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title })}
                  >
                    <div className="grow">
                      <div className="title mini-title" title={task.title}>
                        <span><Icon name="tasks" /></span>
                        <span className="truncate">{task.title}</span>
                      </div>
                    </div>
                    <div className="side">
                      {task.visibility === "archived" && <span className="badge warn">已归档</span>}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        </aside>
      </div>

      {/* 危险操作：删除只在正常状态下出现；已归档的动作在顶部横幅。
          归档的确认弹窗在 SessionHeaderActions 里。 */}
      {purgeOpen && (
        <PermanentDeleteModal
          sessionId={sessionId}
          onClose={() => { setPurgeOpen(false); refresh(); }}
          onDeleted={() => {
            sessionDetailCache.delete(sessionId);
            setPurgeOpen(false);
            goBack({ view: "sessions" });
          }}
        />
      )}

      {trackModalOpen && (
        <SessionTaskTrackingModal
          sessionId={sessionId}
          onClose={() => setTrackModalOpen(false)}
        />
      )}

    </div>
  );
}

function Field({
  label,
  copyValue,
  copyTitle,
  children,
}: {
  label: string;
  copyValue?: string | null;
  copyTitle?: string;
  children: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);

  const onCopy = async () => {
    if (!copyValue) return;
    const ok = await copyToClipboard(copyValue);
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
    showToast(ok ? "已复制" : "复制失败");
  };

  return (
    <>
      <div className="session-field-label-row">
        <span className="muted small" style={{ whiteSpace: "nowrap" }}>{label}</span>
        {copyValue && (
          <button
            type="button"
            className={`btn ghost icon-button small session-field-copy-btn${copied ? " copied" : ""}`}
            title={copied ? "已复制" : (copyTitle ?? `复制${label}`)}
            aria-label={copied ? "已复制" : (copyTitle ?? `复制${label}`)}
            onClick={onCopy}
          >
            <Icon name={copied ? "check" : "copy"} />
          </button>
        )}
      </div>
      <div style={{ minWidth: 0 }}>{children}</div>
    </>
  );
}
