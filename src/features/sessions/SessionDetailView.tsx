import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, contextUpdateErrorCopyText, contextUpdateErrorDetails, copyToClipboard, openPath, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { messageData, type SessionMessageData } from "./SessionMessage";
import SessionSubpageTabs from "./SessionSubpageTabs";
import SessionHeaderActions from "./SessionHeaderActions";
import PermanentDeleteModal from "./PermanentDeleteModal";
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
  type SessionContextFields,
  type SessionContextView,
  type SessionDetail,
  type Workstream,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Session Detail：一个逻辑会话（用户可感知、可 Resume 的主会话）的四块内容——
 * Conversation（只有 user/assistant prose）、Execution Info、Source / Lifecycle、Context / Owner。
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
  const [ownerOpen, setOwnerOpen] = useState(false);
  /** Context：纯读取的四字段摘要 + 显式「生成 / 更新摘要」。 */
  const [sessionCtx, setSessionCtx] = useState<SessionContextView | null>(null);
  const [ctxBusy, setCtxBusy] = useState(false);
  const [ctxReadError, setCtxReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [ctxError, setCtxError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const currentSessionId = useRef(sessionId);
  // 回收站动作（Session Lifecycle & Deletion）：执行中的 busy、
  // 以及从详情页直接发起的删除 Modal。移入回收站的确认与执行在
  // SessionHeaderActions（三个子页共用）。
  const [trashBusy, setTrashBusy] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字；
   * 派生链自带名字，正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  useEffect(() => {
    setCtxError(null);
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
  const fallbackDisplayTitle = fallbackRawTitle ? (
    fallbackRawTitle === UNTITLED_SESSION ? "未命名会话" : fallbackRawTitle
  ) : (
    <span
      className="skeleton"
      style={{
        display: "inline-block",
        width: 140,
        height: 20,
        borderRadius: "var(--radius-sm)",
        verticalAlign: "middle",
      }}
    />
  );

  const fallbackHeaderTitle = failed ? (
    "读取会话失败"
  ) : fallbackAgent ? (
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
        <PageHeader title={fallbackHeaderTitle}>
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
  const { session, owner_workstream } = detail;

  /** 一次点击 → 最多一次模型调用 → 一份新的四字段摘要。失败按后端原因给可行动文案。 */
  const updateSummary = async () => {
    if (ctxBusy) return;
    const updateSessionId = sessionId;
    setCtxBusy(true);
    setCtxReadError(null);
    setCtxError(null);
    try {
      const out = await api.updateSessionContext(updateSessionId);
      if (currentSessionId.current !== updateSessionId) return;
      showToast(
        out.status === "no_change"
          ? "没有新内容，摘要保持不变"
          : out.status === "partial"
            ? "已更新摘要（还有内容待下次更新）"
            : "已更新摘要",
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

  /** 复制当前摘要：四字段按可读文本整段给出去。 */
  const copyContext = async () => {
    if (!sessionCtx?.fields) return;
    const ok = await copyToClipboard(sessionContextText(sessionCtx.fields));
    showToast(ok ? "已复制 Context" : "复制失败，请手动选中文字复制");
  };

  const title = sessionDisplayTitle(session.title);
  const untitled = title === UNTITLED_SESSION;
  const cwd = (session.cwd ?? "").trim();
  /** 单一生命周期权威：null = 正常，时间戳 = 在回收站。 */
  const trashed = session.trashed_at !== null;

  /** 从回收站恢复：trashed_at 清空后本页就地回到正常形态。 */
  const doRestore = async () => {
    if (trashBusy) return;
    setTrashBusy(true);
    try {
      await api.restoreSession(sessionId);
      showToast("已恢复");
      refresh();
    } catch (e) {
      console.error(e);
      showToast(String(e));
    } finally {
      setTrashBusy(false);
    }
  };
  /** 派生链自带的路径与 Project。 */
  const workspacePath = detail.workspace_path;
  const projectIdOnlyCell = projectCellFor(session, projectNameById);
  const derivedProjectName = workspacePath && workspacePath.project_name.trim() !== ""
    ? workspacePath.project_name
    : null;
  /** 这条会话「属于」哪个项目：派生链优先，退回会话行上缓存的 project_id。 */
  const sessionProjectId = workspacePath?.project_id ?? session.project_id ?? null;

  /** 设置所属任务：只改 `sessions.owner_workstream_id`，不碰工作路径与 Project。 */
  const setOwner = async (workstreamId: string | null) => {
    await api.setSessionOwnerWorkstream(sessionId, workstreamId);
    refresh();
    setOwnerOpen(false);
  };

  /** Conversation：只有 root 的 user/assistant prose。 */
  const messages: SessionMessageData[] = detail.messages.map((m) => messageData(m, session.agent));

  /** 按钮只在真的有内容可更新时出现：需要已读到 Context、有消息，且无摘要或有增量。
      回收站中的会话在后端冻结了上下文提取，所以这里也不给入口。 */
  const canUpdateSummary =
    !trashed
    && sessionCtx !== null
    && messages.length > 0
    && (sessionCtx.fields === null || sessionCtx.pending);

  /** 源会话：会话自己的源文件 + 详情加载时的新鲜结论。 */
  const sourceMissing = detail.source_status === "missing";
  const sourceUnavailable = detail.source_status === "unavailable";
  /** 源在，路径本身才是入口；源不在就只是可复制的文本，点了也只会报错。 */
  const canRevealSource = !sourceMissing && !sourceUnavailable;

  const revealSource = async () => {
    try {
      await api.revealSessionSource(sessionId);
    } catch (e) {
      showToast(`定位源会话失败：${String(e)}`);
    }
  };

  /** Resume 的门：源 missing / unavailable 时禁用，并说清为什么。 */
  const resumeDisabled = !detail.can_resume;
  const resumeTitle = detail.can_resume
    ? "继续会话"
    : sourceMissing
      ? "源会话已不存在，无法继续这个会话"
      : "无法确认源会话状态，暂时不能继续";

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
            {/* 子页切换：概览（本页）/ 对话 / 终端。终端段按会话格式出现，
                置灰门槛与 Resume 同源（can_resume）。 */}
            <SessionSubpageTabs
              sessionId={sessionId}
              entry={undefined}
              agent={session.agent}
              sourceKind={session.source_kind}
              terminalGate={resumeDisabled ? resumeTitle : null}
              navigate={navigate}
            />
            <SessionHeaderActions
              sessionId={sessionId}
              agent={session.agent}
              sourceKind={session.source_kind}
              title={title}
              trashed={trashed}
              onChanged={refresh}
            />
          </>
        )}
      />

      {/* 回收站横幅：Trash 是唯一门槛，恢复与删除都可用；删除的结果由弹窗读源状态后说明。 */}
      {trashed && (
        <div className="session-trash-banner">
          <div style={{ minWidth: 0 }}>
            <b>该会话在回收站中</b>
            <div className="small muted" style={{ marginTop: 2 }}>
              移入回收站：{formatDateTime(session.trashed_at)}。它已从常用列表中移除，
              上下文提取在此冻结；Agent 原始会话不会被删除，随时可以恢复。
            </div>
          </div>
          <div className="row" style={{ flex: "none", gap: 8 }}>
            <button className="btn small" disabled={trashBusy} onClick={doRestore}>恢复</button>
            <button className="btn small" disabled={trashBusy} onClick={() => setPurgeOpen(true)}>
              删除…
            </button>
          </div>
        </div>
      )}

      {/* 右栏承载只读事实（Agent、条数、会话信息），左栏是内容本身（所属任务、消息）。 */}
      <div className="task-detail-layout">
      <div className="task-detail-main">
      <div className="row between" style={{ marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>所属任务</div>
      </div>
      {/* 一次最多一个 Owner：要么一条任务，要么「未归属任务」。
          没有「再添加一条」的入口——更换与清空都在这一个入口里。 */}
      {owner_workstream ? (
        <div
          className="rail-row"
          role="link"
          tabIndex={0}
          title={`打开任务：${owner_workstream.title}`}
          onClick={() => navigate({ view: "workstream", workstreamId: owner_workstream.id })}
          onKeyDown={(e) => {
            if (e.key === "Enter" && e.target === e.currentTarget) {
              navigate({ view: "workstream", workstreamId: owner_workstream.id });
            }
          }}
        >
          <div className="rail-main">
            <div className="rail-title" style={{ overflowWrap: "anywhere" }}>
              {owner_workstream.title.trim() || "未命名任务"}
            </div>
          </div>
          <button
            className="btn small"
            onClick={(e) => {
              e.stopPropagation();
              setOwnerOpen(true);
            }}
          >
            更改
          </button>
        </div>
      ) : (
        <div className="rail-row">
          <div className="rail-main">
            <div className="rail-title muted">未归属任务</div>
          </div>
          <button className="btn small" onClick={() => setOwnerOpen(true)}>选择</button>
        </div>
      )}

      {/* Context：四字段只读 + 复制 + 显式「生成 / 更新摘要」。
          只有当有消息、且（还没有摘要 或 有新消息待并入）时才出现按钮——
          没有内容可更新时不摆一个按不动的按钮。 */}
      <div className="row between" style={{ marginTop: 34, alignItems: "center" }}>
        <div className="section-label" style={{ margin: 0 }}>Context</div>
        <div className="row" style={{ gap: 8 }}>
          <button
            className="btn small ghost"
            disabled={!sessionCtx?.fields}
            title={sessionCtx?.fields ? "复制当前摘要" : "还没有摘要"}
            onClick={copyContext}
          >
            复制
          </button>
          {canUpdateSummary && (
            <button className="btn small primary" disabled={ctxBusy} onClick={updateSummary}>
              {ctxBusy ? "更新中…" : sessionCtx?.fields ? "更新摘要" : "生成摘要"}
            </button>
          )}
        </div>
      </div>
      {sessionCtx?.pending && (
        <div className="small muted" style={{ marginTop: 4 }}>
          有新消息尚未并入摘要{sessionCtx.fields === null ? "，点击「生成摘要」" : "，点击「更新摘要」"}。
        </div>
      )}
      {ctxReadError && (
        <div className="badge warn" style={{ marginTop: 8, overflowWrap: "anywhere" }}>
          {ctxReadError.message}
        </div>
      )}
      {ctxError && (
        <div className="badge warn" style={{ marginTop: 8, overflowWrap: "anywhere", display: "flex", gap: 8, alignItems: "center" }}>
          <span>{ctxError.message}{ctxError.operationId ? ` · 操作 ID ${ctxError.operationId}` : ""}</span>
          {ctxError.operationId && (
            <button className="btn small ghost" onClick={async () => {
              const ok = await copyToClipboard(contextUpdateErrorCopyText(ctxError));
              showToast(ok ? "已复制错误详情" : "复制失败，请手动复制错误详情");
            }}>复制错误详情</button>
          )}
        </div>
      )}
      {sessionCtx?.fields ? (
        <div style={{ display: "grid", gap: 12, marginTop: 10, maxWidth: "72ch" }}>
          <ContextField label="Summary / Current State">
            {sessionCtx.fields.summary_current_state.trim() || "—"}
          </ContextField>
          <ContextField label="Decisions">
            <ContextList items={sessionCtx.fields.decisions} />
          </ContextField>
          <ContextField label="Open Questions">
            <ContextList items={sessionCtx.fields.open_questions} />
          </ContextField>
          <ContextField label="Next Steps">
            <ContextList items={sessionCtx.fields.next_steps} />
          </ContextField>
        </div>
      ) : (
        <div className="l1-none" style={{ marginTop: 8 }}>
          {messages.length === 0 ? "还没有摘要；先同步消息后即可生成。" : "还没有摘要。"}
        </div>
      )}

      {/* 详情只预览最近 10 条：整段会话在「查看全部会话」里按需向前翻页读。
          条数用 ingested_message_sequence（当前会话总数），而不是这里的条数。 */}
      <div className="row between" style={{ marginTop: 34, alignItems: "center" }}>
        <div className="section-label" style={{ margin: 0 }}>会话消息</div>
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
            查看全部会话（共 {detail.ingested_message_sequence} 条）
          </button>
        )}
      </div>
      {messages.map((m) => <SessionMessage key={m.sequence} msg={m} />)}
      {detail.ingested_message_sequence > messages.length && (
        <p className="muted small" style={{ marginTop: 10 }}>
          以上是最近 {messages.length} 条。
        </p>
      )}
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

      <aside className="task-detail-aside">
      {/* 只读事实一律在右栏，并且与另外两个详情页同形：rail-section + section-label，
          直接用展开的正文，不用 <details>——默认收起把这页最有用的事实藏了起来。 */}
      <section className="rail-section">
      <div className="section-label">会话信息</div>
      <div>
        <span>{agentDisplayLabel(session.agent)}</span>
      </div>
      {/* 消息条数在下面的统计里有，这里只留状态徽标；两个都没有就不摆空行。 */}
      {(untitled || trashed) && (
        <div className="row" style={{ gap: 8, marginTop: 6, flexWrap: "wrap" }}>
          {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
          {trashed && <span className="badge warn">回收站</span>}
        </div>
      )}

      {/* 执行事实：一次会话「在什么时候、哪个目录、源在哪里」。值可整段选中并复制。 */}
      <div className="session-info-fields">
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
        <Field label="项目">
          {workspacePath ? (
            derivedProjectName ? (
              <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                <button
                  className="link"
                  title={`打开项目：${derivedProjectName}`}
                  onClick={() => navigate({ view: "project", projectId: workspacePath.project_id })}
                >
                  {derivedProjectName}
                </button>

              </span>
            ) : (
                  <span className="muted small">这条工作路径所属的项目记录暂时读不到。</span>
            )
          ) : session.project_id ? (
            <span className="muted small" style={{ wordBreak: "break-word" }}>
              {projectIdOnlyCell.text}
              {" —— "}
              {projectIdOnlyCell.hint}
            </span>
          ) : (
            <span className="muted small">
              {cwd === ""
                ? "没有记录过工作目录，所以没有项目。"
                : "工作路径还没有登记，所以暂时没有项目。"}
            </span>
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
                className="link mono"
                title={`${workspacePath.canonical_path} · 点击在文件管理器中打开`}
                style={{ minWidth: 0, wordBreak: "break-all", textAlign: "left" }}
                onClick={() => void openPath(workspacePath.canonical_path)}
              >
                {workspacePath.canonical_path}
              </button>
              {cwd !== "" && cwd !== workspacePath.canonical_path && (
                <div className="muted small" style={{ marginTop: 4 }}>
                  Agent 原始记录：
                  <button
                    type="button"
                    className="link mono"
                    title={`${cwd} · 点击在文件管理器中打开`}
                    style={{ textAlign: "left", wordBreak: "break-all" }}
                    onClick={() => void openPath(cwd)}
                  >
                    {cwd}
                  </button>
                </div>
              )}
              {!workspacePath.exists && (
                <div style={{ marginTop: 4 }}>
                  <span
                    className="badge warn"
                    title="最近一次目录检查时在磁盘上找不到这个目录。工作路径的身份由路径本身决定，不靠目录存在与否；目录回来时仍然对上同一条工作路径。"
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
                className="link mono"
                title={`${cwd} · 点击在文件管理器中打开`}
                style={{ minWidth: 0, wordBreak: "break-all", textAlign: "left" }}
                onClick={() => void openPath(cwd)}
              >
                {cwd}
              </button>
              <div className="muted small" style={{ marginTop: 4 }}>
                这个目录还没有被登记成工作路径 —— NoEnding 会在下一次目录同步后自动补上，不需要手工操作。
              </div>
            </>
          ) : (
            <span className="muted">{NO_CWD}（该会话的原始记录里没有目录信息）</span>
          )}
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
              title={`${session.source_path} · Agent 保存的源会话 · 点击在文件管理器中显示`}
              style={{ minWidth: 0, wordBreak: "break-all", textAlign: "left" }}
              onClick={() => void revealSource()}
            >
              {session.source_path}
            </button>
          ) : (
            <span
              className="mono"
              title={`${session.source_path} · Agent 保存的源会话`}
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
            title="Root 成员在 Agent 侧的会话身份；Resume 与 LaunchIntent 匹配的唯一依据"
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
                onClick={() => navigate({ view: "session", sessionId: detail.forked_from!.id })}
              >
                分叉自：{sessionDisplayTitle(detail.forked_from.title)}
                {detail.forked_from.trashed_at !== null && (
                  <span className="badge warn" style={{ marginLeft: 6 }}>回收站</span>
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

      </aside>
      </div>

      {/* 危险操作：删除只在正常状态下出现；回收站里的动作在顶部横幅。
          移入回收站的确认弹窗在 SessionHeaderActions 里。 */}
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

      {ownerOpen && (
        <OwnerPickerModal
          currentOwnerId={owner_workstream?.id ?? null}
          projectId={sessionProjectId}
          onClose={() => setOwnerOpen(false)}
          onSubmit={setOwner}
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
    showToast(ok ? "已复制到剪贴板" : "复制失败，请手动选中文字复制");
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

/** 一个只读 Context 字段：标签 + 值。 */
function ContextField({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="muted small">{label}</div>
      <div className="small" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{children}</div>
    </div>
  );
}

/** 字符串数组字段：空数组不冒称有内容。 */
function ContextList({ items }: { items: string[] }) {
  if (items.length === 0) return <span className="muted">—</span>;
  return (
    <ul style={{ margin: 0, paddingLeft: 18 }}>
      {items.map((item, i) => <li key={i}>{item}</li>)}
    </ul>
  );
}

/** 复制用的纯文本渲染：四字段按可读顺序整段给出去。 */
function sessionContextText(f: SessionContextFields): string {
  const list = (xs: string[]) => (xs.length > 0 ? xs.map((x) => `- ${x}`).join("\n") : "—");
  return [
    "Summary / Current State:",
    f.summary_current_state.trim() || "—",
    "",
    "Decisions:",
    list(f.decisions),
    "",
    "Open Questions:",
    list(f.open_questions),
    "",
    "Next Steps:",
    list(f.next_steps),
  ].join("\n");
}

/**
 * 选择所属任务：单选，可清空为「未归属」。
 *
 * 候选按「当前 Project → 其他任务」分组，与工作目录有路径关联的任务优先；
 * 后端不强制这个限制，其他任务仍可选中。归档任务不接收新的归属。
 */
function OwnerPickerModal({ currentOwnerId, projectId, onClose, onSubmit }: {
  /** 当前所属任务；null = 未归属。 */
  currentOwnerId: string | null;
  /** null = 这条会话没有可解析的项目，此时不分组，只列全部任务。 */
  projectId: string | null;
  onClose: () => void;
  onSubmit: (workstreamId: string | null) => Promise<void>;
}) {
  const [tasks, setTasks] = useState<Workstream[] | null>(null);
  const [projectTaskIds, setProjectTaskIds] = useState<string[]>([]);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState<string | null>(currentOwnerId);
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const [failText, setFailText] = useState("");

  useEffect(() => {
    let active = true;
    setError(false);
    Promise.all([
      api.listWorkstreams(),
      projectId ? api.listWorkstreams(projectId) : Promise.resolve([]),
    ])
      .then(([all, inProject]) => {
        if (!active) return;
        setTasks(all.filter((w) => w.visibility === "normal"));
        setProjectTaskIds(inProject.map((w) => w.id));
      })
      .catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [projectId, attempt]);

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setFailText("");
    try {
      await onSubmit(selected);
    } catch (e) {
      setFailText(String(e));
    } finally {
      setBusy(false);
    }
  };

  const option = (id: string | null, title: string) => (
    <label className="existing-path-option" key={id ?? "none"}>
      <input
        type="radio"
        name="owner-workstream"
        checked={selected === id}
        onChange={() => setSelected(id)}
      />
      <span className="existing-path-info" title={title}>{title}</span>
    </label>
  );

  const otherTasks = (tasks ?? []).filter((w) => !projectTaskIds.includes(w.id));
  const projectTasks = (tasks ?? []).filter((w) => projectTaskIds.includes(w.id));

  return (
    <Modal title="选择所属任务" onClose={onClose}>
      <div className="existing-path-list" role="radiogroup" aria-label="所属任务">
        {error ? (
          <div role="alert">
            读取任务失败 <button className="btn small" onClick={() => setAttempt((n) => n + 1)}>重试</button>
          </div>
        ) : !tasks ? (
          <div role="status">加载中…</div>
        ) : (
          <>
            {option(null, "未归属")}
            {projectTasks.length > 0 ? (
              <>
                <div className="muted small" style={{ marginTop: 8 }}>当前项目</div>
                {projectTasks.map((w) => option(w.id, w.title.trim() || "未命名任务"))}
              </>
            ) : null}
            {projectTasks.length > 0 && otherTasks.length > 0 ? (
              <div className="muted small" style={{ marginTop: 8 }}>其他任务</div>
            ) : null}
            {otherTasks.map((w) => option(w.id, w.title.trim() || "未命名任务"))}
          </>
        )}
      </div>
      {failText && <div className="badge warn" style={{ marginTop: 8 }}>{failText}</div>}
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy || !tasks || selected === currentOwnerId} onClick={submit}>
          {busy ? "保存中…" : "保存"}
        </button>
      </div>
    </Modal>
  );
}
