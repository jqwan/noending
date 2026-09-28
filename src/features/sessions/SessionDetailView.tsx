import Icon from "../../components/Icon";
import { useViewState } from "../../hooks/useViewState";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, contextUpdateErrorCopyText, contextUpdateErrorDetails, copyToClipboard, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { messageData, type SessionMessageData } from "./SessionMessage";
import ResumeSessionModal from "./ResumeSessionModal";
import PermanentDeleteModal from "./PermanentDeleteModal";
import {
  agentDisplayLabel,
  cwdDisplayLabel,
  formatDateTime,
  projectCellFor,
  sessionDisplayTitle,
  shrinkMiddle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import {
  type Project,
  type SessionContextFields,
  type SessionContextView,
  type SessionDetail,
  type SessionMemberRelation,
  type Workstream,
} from "../../types";
import type { Route } from "../../app/routes";

/** 详情里的一个执行成员：member 行 + 查询时带出的统计快照。 */
type DetailMember = SessionDetail["members"][number];

const MEMBER_ID_WIDTH = 24;
/** 成员行里路径的显示宽度：尾段（文件名）最有信息量，按路径规则中段省略。 */
const MEMBER_PATH_WIDTH = 22;

/**
 * Session Detail：一个逻辑会话（用户可感知、可 Resume 的主会话）的四块内容——
 * Conversation（只有 user/assistant prose）、Execution Info、Source / Lifecycle、Context / Owner。
 * 没有 parent/children Session 链接：执行图以 members 呈现，唯一的会话→会话链接是 fork 来源。
 */
export default function SessionDetailView({ sessionId, navigate, goBack }: {
  sessionId: string;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [ownerOpen, setOwnerOpen] = useState(false);
  const [resumeOpen, setResumeOpen] = useState(false);
  /** Context：纯读取的四字段摘要 + 显式「生成 / 更新摘要」。 */
  const [sessionCtx, setSessionCtx] = useState<SessionContextView | null>(null);
  const [ctxBusy, setCtxBusy] = useState(false);
  const [ctxReadError, setCtxReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [ctxError, setCtxError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const currentSessionId = useRef(sessionId);
  currentSessionId.current = sessionId;
  // 回收站动作（Session Lifecycle & Deletion）：确认弹窗、执行中的 busy、
  // 以及从详情页直接发起的删除 Modal。
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);
  /** 成员各自的数字默认不画：先给结构与来源，要横向比各成员时再点开。 */
  const [showMemberStats, setShowMemberStats] = useViewState("session.memberStats", false);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字；
   * 派生链自带名字，正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  useEffect(() => {
    setCtxError(null);
    setCtxReadError(null);
    setCtxBusy(false);
  }, [sessionId]);

  /** 打开 / 刷新这一页：详情与 Session Context 都是纯读取。 */
  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => { setDetail(d); setFailed(false); })
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

  /**
   * 没拿到数据时也要把 PageHeader 画出来：标题行吸在应用标题栏那条 band 上，
   * 加载态若不渲染它，整条标题栏会先消失再补回来，比"内容区里一行加载中"显眼得多。
   */
  if (!detail) {
    return (
      <div className="main narrow">
        <PageHeader title={failed ? "读取会话失败" : "加载中…"}>
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
      </div>
    );
  }
  const { session, owner_workstream } = detail;
  /** 没有子/边成员时整节不出现：那时清单就是一行 root，汇总里的「规模」已经把话说完了。 */
  const hasSubMembers = detail.members.some((m) => m.relation !== "root");

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

  /** 移入回收站：全局隐藏，不删任何数据。成功后返回上一个界面。 */
  const doTrash = async () => {
    if (trashBusy) return;
    setTrashBusy(true);
    try {
      await api.trashSession(sessionId);
      showToast("已移入回收站");
      setConfirmTrash(false);
      goBack({ view: "sessions" });
    } catch (e) {
      console.error(e);
      showToast(`移入回收站失败：${String(e)}`);
      setTrashBusy(false);
    }
  };

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

  /**
   * 源会话：Root 成员的源文件 + 详情加载时的新鲜结论。
   * members 里没有 root 行本身即异常，按 unavailable 对待。
   */
  const rootMember = detail.members.find((m) => m.relation === "root") ?? null;
  const sourceMissing = rootMember !== null && detail.root_source_status === "missing";
  const sourceUnavailable = rootMember === null || detail.root_source_status === "unavailable";

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
    <div className="main narrow">
      <PageHeader
        title={title}
        actions={trashed ? (
          // 回收站中的会话：后端拒绝 Resume——如实呈现为不可用。
          <button className="btn ghost icon-button" disabled
            aria-label="继续" title="回收站中的会话不能继续；先在上面的横幅里恢复它。">
            <Icon name="play" />
          </button>
        ) : (
          <>
            <button className="btn ghost icon-button" aria-label="移入回收站" title="移入回收站" onClick={() => setConfirmTrash(true)} disabled={trashBusy}>
              <Icon name="trash" />
            </button>
            <button className="btn ghost icon-button" aria-label="重新读取" title="重新读取本地数据（不会触发摄入）" onClick={refresh}><Icon name="refresh" /></button>
            <button className="btn ghost icon-button" aria-label="继续" title={resumeTitle} onClick={() => setResumeOpen(true)} disabled={resumeDisabled}>
              <Icon name="play" />
            </button>
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
        <div className="rail-row">
          <div className="rail-main">
            <button
              className="link"
              style={{ textAlign: "left", overflowWrap: "anywhere" }}
              title={`打开任务：${owner_workstream.title}`}
              onClick={() => navigate({ view: "workstream", workstreamId: owner_workstream.id })}
            >
              {owner_workstream.title.trim() || "未命名任务"}
            </button>
          </div>
          <button className="btn small" onClick={() => setOwnerOpen(true)}>更改</button>
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
          {messages.length === 0 ? "还没有摘要；先摄入消息后即可生成。" : "还没有摘要。"}
        </div>
      )}

      {/* 详情只预览最近 10 条：整段会话在「查看全部会话」里按需向前翻页读。
          条数用 ingested_message_sequence（当前会话总数），而不是这里的条数。 */}
      <div className="row between" style={{ marginTop: 34, alignItems: "center" }}>
        <div className="section-label" style={{ margin: 0 }}>会话消息</div>
        {detail.ingested_message_sequence > 0 && (
          <button
            className="btn small ghost"
            onClick={() => navigate({ view: "session", sessionId, entry: "conversation" })}
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
          还没有摄入消息。
          <div className="small" style={{ marginTop: 4 }}>
            重新读取以加载已摄入的原始会话。
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
      <div className="row" style={{ gap: 6 }}>
        <AgentIcon agent={session.agent} />
        <span>{agentDisplayLabel(session.agent)}</span>
      </div>
      <div className="row" style={{ gap: 8, marginTop: 6, flexWrap: "wrap" }}>
        {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
        {trashed && <span className="badge warn">回收站</span>}
        <span className="small muted">{messages.length > 0 ? `${messages.length} 条消息` : "尚无消息"}</span>
      </div>

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
        <Field label="工作目录">
          {workspacePath ? (
            <>
              <CopyValue
                value={workspacePath.canonical_path}
                mono
                title={`${workspacePath.canonical_path} · NoEnding 识别工作位置用的规范化路径`}
              />
              {cwd !== "" && cwd !== workspacePath.canonical_path && (
                <div className="muted small" style={{ marginTop: 4 }}>
                  Agent 原始记录：{cwd}
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
              <CopyValue value={cwd} title={`${cwd} · Agent 原始记录里的 cwd，是这条会话自己的事实`} />
              <div className="muted small" style={{ marginTop: 4 }}>
                这个目录还没有被登记成工作路径 —— NoEnding 会在下一次目录扫描后自动补上，不需要手工操作。
              </div>
            </>
          ) : (
            <span className="muted">{NO_CWD}（该会话的原始记录里没有目录信息）</span>
          )}
        </Field>
        {/* 源会话：Root 成员的源文件，不再是 session.raw_path。
            状态是详情加载时对源的新鲜结论，missing / unavailable 都如实说出。 */}
        <Field label="源会话">
          {rootMember ? (
            <>
              <CopyValue
                value={rootMember.source_path}
                mono
                title={`${rootMember.source_path} · Agent 保存的 Root 源会话`}
              />
              <button
                className="link"
                onClick={revealSource}
                disabled={sourceMissing || sourceUnavailable}
                title={sourceMissing ? "源会话文件已不存在" : sourceUnavailable ? "无法确认源会话文件状态" : "在文件管理器中显示源会话文件"}
              >
                在文件管理器中显示
              </button>
              {sourceMissing && (
                <div className="session-source-warning">源会话已不存在</div>
              )}
              {sourceUnavailable && (
                <div className="session-source-warning">无法确认源会话状态</div>
              )}
            </>
          ) : (
            <>
              <span className="muted small">这个会话没有 Root 成员记录。</span>
              <div className="session-source-warning">无法确认源会话状态</div>
            </>
          )}
        </Field>
        <Field label="会话 ID">
          <CopyValue value={session.id} mono />
        </Field>
        <Field label="Agent 会话 ID">
          <CopyValue value={session.root_agent_session_id} mono
            title="Root 成员在 Agent 侧的会话身份；Resume 与 LaunchIntent 匹配的唯一依据" />
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

      {/* 执行成员：这次执行由哪些成员组成——根 / 子 / 边、各自的身份与来源。
          成员不是链接：它们是 Agent 内部的执行单元，不是另一个 Session 页面。
          root 的源不在这里重复画（它在「会话信息 · 源会话」里，带着删除流程要用的结论）；
          成员各自的数字默认不画，开关归这一节；没有子/边成员时整节不出现。 */}
      {hasSubMembers && (
        <section className="rail-section">
        <div className="row between" style={{ alignItems: "center" }}>
          <div className="section-label" style={{ margin: 0 }}>执行成员</div>
          <button
            className="btn small ghost"
            aria-expanded={showMemberStats}
            title="显示每个成员自己的数字（计数、tokens、成本）"
            onClick={() => setShowMemberStats((on) => !on)}
          >
            {showMemberStats ? "隐藏统计" : "显示统计"}
          </button>
        </div>
        <MemberTree members={detail.members} costUnit={detail.cost_unit} showStats={showMemberStats} />
        </section>
      )}

      {/* 执行统计：会话级汇总——各成员自己份额的和。 */}
      <section className="rail-section">
      <div className="section-label">执行统计</div>
      <ExecutionStats stats={detail.stats} costUnit={detail.cost_unit} />
      </section>
      </aside>
      </div>

      {/* 危险操作：只在正常状态下出现；回收站里的动作在顶部横幅。 */}
      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => { if (!trashBusy) setConfirmTrash(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会从 Sessions 列表、搜索与继续入口中消失，出现在 Sessions 页的「回收站」里。
          </p>
          {/* 要求把两个概念摆在同一处明确区分：「从任务移除」只改这条会话的
              所属任务（详情页的「更改 / 选择」），这里是全局回收站。 */}
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从任务移除</b> = 只修改这条会话的所属任务，会话本身留在列表里。
            </p>
            <p style={{ margin: 0 }}>
              <b>移入回收站</b> = 在 NoEnding 中全局隐藏该 Session。Agent 原始会话不会被删除。
            </p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmTrash(false)} disabled={trashBusy}>取消</button>
            <button className="btn primary" onClick={doTrash} disabled={trashBusy}>
              {trashBusy ? "处理中…" : "移入回收站"}
            </button>
          </div>
        </Modal>
      )}

      {purgeOpen && (
        <PermanentDeleteModal
          sessionId={sessionId}
          onClose={() => { setPurgeOpen(false); refresh(); }}
          onDeleted={() => { setPurgeOpen(false); goBack({ view: "sessions" }); }}
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
      {resumeOpen && (
        <ResumeSessionModal
          sessionId={sessionId}
          onClose={() => setResumeOpen(false)}
        />
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <div className="muted small" style={{ whiteSpace: "nowrap" }}>{label}</div>
      <div style={{ minWidth: 0 }}>{children}</div>
    </>
  );
}

/** 数值 → 千位分隔；null = 源不提供，显示「—」。 */
const fmtCount = (n: number | null): string => (n === null ? "—" : n.toLocaleString("en-US"));

/**
 * 成本带单位：各 Agent 有各自的单位（Pi 是 USD、Qoder 是 credits），
 * 不写单位就等于替源安了一个它没用的单位。
 */
function formatCost(cost: number, unit: string | null): string {
  return `${cost.toLocaleString("en-US", { maximumFractionDigits: 4 })}${unit ? ` ${unit}` : ""}`;
}

/**
 * 会话级执行统计：标签列 + 两列数值。执行图的规模永远显示；
 * 消息、token、成本只在数据真的在场上时出现——有数据才显示，没有就不画一行「0」去冒称观测。
 * 组名叫「消息构成」：这一组描述整个执行图的**源消息**构成——用户、助手的对话轮次，
 * 以及工具调用、压缩、协同（子代理转述）这些源记录。每个成员各自统计，这里是汇总；
 * 单个成员自己的份额在「执行成员」里该成员那一行。
 * 成本带单位：数字各 Agent 有自己的单位（Pi 是 USD，Qoder 是 credits），不写单位就等于
 * 替源安了一个它没用的单位。
 *
 * 不用「值 · 值 · 值」串烧：右栏只有 240–300px，十来个数字挤一行必然折行，
 * 而折回来的第二行顶格起排、看起来像新的一条。两列网格让每个数各占一格，
 * 折行和错位都不会发生，标签也把「工具 4」这类孤零零的数字放回了它的组里。
 */
function ExecutionStats({
  stats,
  costUnit,
}: {
  stats: SessionDetail["stats"];
  costUnit: SessionDetail["cost_unit"];
}) {
  const messageCells: string[] = [];
  if (stats.user_message_count > 0) messageCells.push(`用户 ${stats.user_message_count}`);
  if (stats.assistant_message_count > 0)
    messageCells.push(`助手 ${stats.assistant_message_count}`);
  if (stats.tool_call_count > 0) messageCells.push(`工具 ${stats.tool_call_count}`);
  if (stats.compaction_count > 0) messageCells.push(`压缩 ${stats.compaction_count}`);
  if (stats.side_activity_count > 0) messageCells.push(`协同 ${stats.side_activity_count}`);

  const tokenCells: string[] = [];
  if (stats.input_tokens !== null) tokenCells.push(`输入 ${fmtCount(stats.input_tokens)}`);
  if (stats.output_tokens !== null) tokenCells.push(`输出 ${fmtCount(stats.output_tokens)}`);
  if (stats.cached_tokens !== null) tokenCells.push(`缓存 ${fmtCount(stats.cached_tokens)}`);
  if (stats.reasoning_tokens !== null) tokenCells.push(`推理 ${fmtCount(stats.reasoning_tokens)}`);

  // 不显示"Session 的 model/provider/effort"：一个 Logical Session 可能中途切模型，
  // 不存在天然的 Session model；消息级模型标签在消息上。

  const group = (label: string, cells: string[], key: string) => (
    <Fragment key={key}>
      <div className="muted small exec-label">{label}</div>
      <div className="exec-pairs">
        {cells.map((c) => <span key={c}>{c}</span>)}
      </div>
    </Fragment>
  );

  return (
    <div className="exec-metrics">
      {group("规模", [
        `${stats.member_count} 个成员`,
        `${stats.child_count} 子`,
        `${stats.side_count} 边`,
        `深度 ${stats.max_depth}`,
      ], "tree")}
      {messageCells.length > 0 && group("消息构成", messageCells, "messages")}
      {tokenCells.length > 0 && group("Tokens", tokenCells, "tokens")}
      {stats.cost !== null && group("成本", [
        formatCost(stats.cost, costUnit),
      ], "cost")}
    </div>
  );
}

const RELATION_LABELS: Record<SessionMemberRelation, string> = {
  root: "根",
  child: "子",
  side: "边",
};

/**
 * 一行成员：关系标签 + 稳定身份（mono，居中断尾，原值在 title 里），其后是源文件（可复制）；
 * `showStats` 打开时才补上这个成员自己的数字。
 *
 * 每个成员——根、子、边——都有自己的份额：消息构成、tokens、成本都是它自己的数，
 * 右栏上面的汇总只是把它们加起来。有数据才画那一行，没有就不拿 0 去冒称观测。
 *
 * 计数不再和身份挤同一行：「根 / 子 / 边」三个标签宽度不同，身份列因此对不齐，
 * 计数又常把这一行挤到换行——树里于是出现一行 46px、一行 19px 的锯齿。
 * 关系标签固定列宽 + 其余各行各占一行后，每行等高、身份列对齐。
 */
function MemberRow({ member, depth, costUnit, showStats }: {
  member: DetailMember;
  depth: number;
  costUnit: string | null;
  showStats: boolean;
}) {
  const s = member.stats;
  const bits: string[] = [];
  const counted = (label: string, value: number | null | undefined) => {
    if (value != null && value > 0) bits.push(`${label} ${value}`);
  };
  counted("用户", s?.user_message_count);
  counted("助手", s?.assistant_message_count);
  counted("工具", s?.tool_call_count);
  counted("压缩", s?.compaction_count);
  counted("协同", s?.side_activity_count);

  // null = 源不提供这一项，与计数同一个判据；成本同样带单位。
  const usage: string[] = [];
  const provided = (label: string, value: number | null | undefined) => {
    if (value != null) usage.push(`${label} ${fmtCount(value)}`);
  };
  provided("输入", s?.input_tokens);
  provided("输出", s?.output_tokens);
  provided("缓存", s?.cached_tokens);
  provided("推理", s?.reasoning_tokens);
  if (s?.cost != null) usage.push(`成本 ${formatCost(s.cost, costUnit)}`);

  return (
    <div className="member-row" style={{ paddingLeft: depth * 16 }}>
      <span className="badge member-rel">{RELATION_LABELS[member.relation]}</span>
      <div className="member-body">
        <div className="mono small member-id" title={member.source_member_id}>
          {shrinkMiddle(member.source_member_id, MEMBER_ID_WIDTH)}
        </div>
        {showStats && bits.length > 0 && <MemberValues values={bits} />}
        {showStats && usage.length > 0 && <MemberValues values={usage} />}
        {/* root 的源在「会话信息 · 源会话」里（那里还带新鲜结论），不重复画。 */}
        {member.relation !== "root" && (
          <div className="small">
            <CopyValue
              label="源"
              value={member.source_path}
              display={cwdDisplayLabel(member.source_path, MEMBER_PATH_WIDTH)}
              truncate
              mono
              title={`${member.source_path} · 这个成员在 Agent 侧的源`}
            />
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * 成员行里的一行数值：两列网格，与上面的汇总组同形——各成员的数字因此在列上对齐，
 * 值内部不折（「推理」「2,100」不能变成两行两个数）。没有的项直接不画，不用 0 占位。
 */
function MemberValues({ values }: { values: string[] }) {
  return (
    <div className="muted exec-pairs">
      {values.map((v) => <span key={v} style={{ whiteSpace: "nowrap" }}>{v}</span>)}
    </div>
  );
}

/**
 * 成员树：root 在顶，child / side 挂在自己的 parent 下，缩进呈现。
 * parent 记录缺席（未摄入或被清理）的成员不能消失——按顶层孤儿如实列出。
 * 常显，不再有展开/收起：这是详情页的一等事实，不是折叠起来的附录。
 */
function MemberTree({ members, costUnit, showStats }: {
  members: DetailMember[];
  costUnit: string | null;
  showStats: boolean;
}) {
  const byParent = useMemo(() => {
    const map = new Map<string, DetailMember[]>();
    const ids = new Set(members.map((m) => m.source_member_id));
    const top: DetailMember[] = [];
    for (const m of members) {
      const parent = m.relation === "root" ? null : m.parent_source_member_id;
      if (parent === null || !ids.has(parent)) top.push(m);
      else {
        const list = map.get(parent) ?? [];
        list.push(m);
        map.set(parent, list);
      }
    }
    return { map, top };
  }, [members]);

  const renderNode = (m: DetailMember, depth: number): React.ReactNode[] => [
    <MemberRow key={m.id} member={m} depth={depth} costUnit={costUnit} showStats={showStats} />,
    ...(byParent.map.get(m.source_member_id) ?? []).flatMap((c) => renderNode(c, depth + 1)),
  ];

  return (
    <div style={{ display: "grid", gap: 10 }}>
      {byParent.top.flatMap((m) => renderNode(m, 0))}
    </div>
  );
}

/**
 * 值 + 复制：报障时用户要能原样给出 Session ID 与工作目录。
 * `display` 只改显示（路径按中段省略，尾段最有信息量），复制与 `title` 仍是原值；
 * 传了 `display` 就配 `truncate`：已经省略过的东西再折行只会把尾段折断。
 */
function CopyValue({ label, value, display, truncate, mono, title }: {
  label?: string;
  value: string;
  display?: string;
  truncate?: boolean;
  mono?: boolean;
  title?: string;
}) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    const ok = await copyToClipboard(value);
    setCopied(ok);
    showToast(ok ? "已复制到剪贴板" : "复制失败，请手动选中文字复制");
    if (ok) setTimeout(() => setCopied(false), 2500);
  };

  return (
    <span className="row" style={{ gap: 8 }}>
      {label && <span className="muted small" style={{ flex: "none" }}>{label}</span>}
      <span
        className={mono ? "mono" : undefined}
        title={title ?? value}
        style={truncate
          ? { minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", userSelect: "all" }
          : { minWidth: 0, wordBreak: "break-all", userSelect: "all" }}
      >
        {display ?? value}
      </span>
      <button className="link" style={{ flex: "none" }} onClick={copy}>
        {copied ? "已复制" : "复制"}
      </button>
    </span>
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
