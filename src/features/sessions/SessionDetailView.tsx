import SessionDetailPanel from "./SessionDetailPanel";
import { DetailLayout, DetailPanelProvider } from "../../components/DetailLayout";
import { DetailContent, DetailSection } from "../../components/DetailSection";
import { useOperationState, refreshOperationStatus } from "../../hooks/useOperationState";
import { MailboxHistory } from "./MailboxMessages";
import { removedTargets } from "../../app/removedTargets";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "./SummaryContent";
import Icon from "../../components/Icon";
import { useCallback, useEffect, useRef, useState } from "react";
import "./sessions.css";
import { api } from "../../api";

import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { contextUpdateErrorCopyText, contextUpdateErrorDetails, copyToClipboard, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { messageData } from "./SessionMessage";
import SessionSubpageTabs from "./SessionSubpageTabs";
import SessionHeaderActions from "./SessionHeaderActions";
import PermanentDeleteModal from "./PermanentDeleteModal";
import { sessionDisplayTitle } from "./SessionTable";
import {
  type Agent,
  type SessionContextView,
  type SessionDetail,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Session Detail：一个逻辑会话（用户可感知、可 Resume 的主会话）的四块内容——
 * Conversation（只有 user/assistant prose）、Execution Info、Source / Lifecycle、Context / task tracking。
 * 唯一的会话→会话链接是 fork 来源。
 */
import { sessionDetailCache, sessionContextCache } from "./sessionDetailCache";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
export { sessionDetailCache, sessionContextCache };

export default function SessionDetailView({
  sessionId,
  initialTitle,
  initialAgent,
  navigate,
}: {
  sessionId: string;
  initialTitle?: string;
  initialAgent?: Agent;
  navigate: (r: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(() => sessionDetailCache.get(sessionId) ?? null);
  const [failed, setFailed] = useState(false);
  const showSkeleton = useLoadingDelay(!detail && !failed);
  /** Current summary content is generated from session history. */
  const [sessionCtx, setSessionCtx] = useState<SessionContextView | null>(() => sessionContextCache.get(sessionId) ?? null);
  const summaryExpansion = useSummaryExpansion(sessionCtx?.fields?.summary);
  const operation = useOperationState("summary", "session", sessionId);
  const [ctxBusy, setCtxBusy] = useState<"update" | "regenerate" | false>(false);
  const [ctxReadError, setCtxReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [ctxError, setCtxError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const currentSessionId = useRef(sessionId);
  // 已归档动作（Session Lifecycle & Deletion）：执行中的 busy、
  // 以及从详情页直接发起的删除 Modal。归档的确认与执行在
  // SessionHeaderActions（三个子页共用）。
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);

  useEffect(() => {
    setCtxError(null);
    setCtxReadError(null);
    setCtxBusy(false);
    if (currentSessionId.current !== sessionId) {
      currentSessionId.current = sessionId;
      setDetail(sessionDetailCache.get(sessionId) ?? null);
      setSessionCtx(sessionContextCache.get(sessionId) ?? null);
    }
  }, [sessionId]);

  /** 打开 / 刷新这一页：详情与 Session Context 都是纯读取。 */
  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => {
        if (removedTargets.sessions.has(sessionId)) return;
        sessionDetailCache.set(sessionId, d);
        setDetail(d);
        setFailed(false);
      })
      .catch((e) => { console.error(e); setFailed(true); });
    api.getSessionContext(sessionId)
      .then((c) => {
        if (removedTargets.sessions.has(sessionId)) return;
        sessionContextCache.set(sessionId, c);
        setSessionCtx(c);
        setCtxReadError(null);
      })
      .catch((e) => { console.error(e); setCtxReadError(contextUpdateErrorDetails(e)); });
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const fallbackAgent = initialAgent;
  const fallbackRawTitle = initialTitle;
  const fallbackDisplayTitle = fallbackRawTitle ? sessionDisplayTitle(fallbackRawTitle) : "";

  /**
   * 没拿到数据时也要把 PageHeader 画出来：标题行吸在应用标题栏那条 band 上，
   * 加载态若不渲染它，整条标题栏会先消失再补回来，比"内容区里一行加载中"显眼得多。
   */

  if (!detail) {
    return (
      <div className="main session-detail" role="status">
        <PageHeader title={fallbackDisplayTitle} icon={fallbackAgent ? <AgentIcon agent={fallbackAgent} size={18} /> : <Icon name="chat" />} navigation={<SessionSubpageTabs sessionId={sessionId} title={fallbackDisplayTitle} agent={fallbackAgent} navigate={navigate} />}>
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
        {!failed && showSkeleton && (
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
    if (ctxBusy || operation.busy || operation.checking) return;
    const updateSessionId = sessionId;
    setCtxBusy(regenerate ? "regenerate" : "update");
    setCtxReadError(null);
    setCtxError(null);
    try {
      const out = regenerate
        ? await api.regenerateSessionContext(updateSessionId)
        : await api.updateSessionContext(updateSessionId);
      if (currentSessionId.current !== updateSessionId) return;
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
      await refreshOperationStatus(true);
      if (currentSessionId.current === updateSessionId) setCtxBusy(false);
    }
  };

  /** 复制当前摘要：保留 Markdown 原文。 */
  const copyContext = async () => {
    if (!sessionCtx?.fields) return;
    const ok = await copyToClipboard(summaryMarkdown(sessionCtx.fields.summary));
    showToast(ok ? "已复制会话摘要" : "复制失败");
  };

  const title = sessionDisplayTitle(session.title);
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
  /** Conversation：只有 root 的 user/assistant prose。仅展示最近 3 条消息。 */
  const messages = detail.messages;
  const recentMessages = messages.slice(-3);
  const hasMessages = messages.length > 0 || detail.ingested_message_sequence > 0;

  /** 更新处理增量；重新生成即使没有增量也可从完整历史重建。归档不影响这两项操作。 */
  const canUpdateSummary =
    sessionCtx !== null
    && hasMessages
    && sessionCtx.pending;
  const canRegenerateSummary = sessionCtx !== null && hasMessages;

  return (
    <DetailPanelProvider kind="session"><div className="main session-detail detail-page">
      <PageHeader
        title={title} icon={<AgentIcon agent={session.agent} size={18} />}
        navigation={<SessionSubpageTabs
              sessionId={sessionId}
              title={title}
              entry={undefined}
              agent={session.agent}
              sourceKind={session.source_kind}
              archived={archived}
              navigate={navigate}
            />}
        actions={(
          <>
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
      <DetailLayout panel={<SessionDetailPanel key={sessionId} detail={detail} navigate={navigate} />}><MailboxHistory sessionId={sessionId} navigate={navigate} />
<DetailSection sectionId="summary" label="会话摘要" title={<>
                <Icon name="spark" />
                <span>会话摘要{sessionCtx?.fields?.summary ? `（${sessionCtx?.fields?.summary.items.length}）` : ""}</span>
              </>} actions={<>
                <SummaryControls expansion={summaryExpansion} />
                {sessionCtx?.fields?.summary && (
                  <button
                    type="button"
                    className="btn ghost icon-button small"
                    title="复制摘要"
                    aria-label="复制摘要"
                    onClick={copyContext}
                  >
                    <Icon name="copy" />
                  </button>
                )}
                {canRegenerateSummary && (
                  <button
                    type="button"
                    className="btn ghost icon-button small"
                    disabled={operation.busy || operation.checking || Boolean(ctxBusy)}
                    title={operation.error ? "无法读取更新状态" : ctxBusy === "regenerate" || operation.busy ? "重新生成中…" : "重新生成"}
                    aria-label="重新生成"
                    onClick={() => updateSummary(true)}
                  >
                    <Icon name="refresh" className={ctxBusy === "regenerate" || (operation.busy && !ctxBusy) ? "spinning" : ""} />
                  </button>
                )}
                {canUpdateSummary && (
                  <button
                    type="button"
                    className="btn primary icon-button small"
                    disabled={operation.busy || operation.checking || Boolean(ctxBusy)}
                    title={operation.error ? "无法读取更新状态" : ctxBusy === "update" || operation.busy ? "更新中…" : "更新摘要"}
                    aria-label="更新摘要"
                    onClick={() => updateSummary()}
                  >
                    <Icon name="spark" className={ctxBusy === "update" || operation.busy ? "spinning" : ""} />
                  </button>
                )}
              </>} status={<>
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
    </>}>

            {sessionCtx && sessionCtx.fields ? (
              <DetailContent label="会话摘要内容" className="md-body">
                <SummaryContent expansion={summaryExpansion}
                 >
                  {sessionCtx.fields?.summary ?? {items:[]}}
                </SummaryContent>
              </DetailContent>
            ) : sessionCtx === null ? (
              <div className="skeleton" style={{ height: 48, borderRadius: "var(--radius-md)" }} />
            ) : (
              <div className="detail-empty">
                {!hasMessages ? "暂无消息" : "暂无摘要"}
              </div>
            )}
          </DetailSection>
<DetailSection sectionId="messages" label="最近消息" title={<>
                <Icon name="chatLines" />
                <span>最近消息</span>
              </>} actions={<>{detail.ingested_message_sequence > 0 && (
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
              )}</>}>

            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {recentMessages.map((m) => (
                <SessionMessage key={m.sequence} msg={messageData(m, session.agent)} ordinal={m.ordinal} />
              ))}
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
          </DetailSection></DetailLayout>

      {/* 危险操作：删除只在正常状态下出现；已归档的动作在顶部横幅。
          归档的确认弹窗在 SessionHeaderActions 里。 */}
      {purgeOpen && (
        <PermanentDeleteModal
          sessionId={sessionId}
          onClose={() => { setPurgeOpen(false); refresh(); }}
          onDeleted={() => {
            sessionDetailCache.delete(sessionId);
            sessionContextCache.delete(sessionId);
            setPurgeOpen(false);
            navigate({ view: "sessions", scope: "archived" });
          }}
        />
      )}

    </div></DetailPanelProvider>
  );
}
