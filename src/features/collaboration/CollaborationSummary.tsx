import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { WorkstreamContextView } from "../../types";
import { DetailContent, DetailSection } from "../../components/DetailSection";
import Icon from "../../components/Icon";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { refreshOperationStatus, useOperationState } from "../../hooks/useOperationState";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "../sessions/SummaryContent";

/** 任务详情与协作室复用一份摘要；阅读和折叠不改变成员的消息阅读位置。 */
export default function CollaborationSummary({ taskId, latestSeq = 0, initiallyCollapsed = false }: {
  taskId: string; latestSeq?: number; initiallyCollapsed?: boolean;
}) {
  const [view, setView] = useState<WorkstreamContextView | null>(null);
  const [busy, setBusy] = useState<"update" | "regenerate" | null>(null);
  const [error, setError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const live = useRef(true), request = useRef(0), busyRef = useRef(false);
  const expansion = useSummaryExpansion(view?.summary);
  const operation = useOperationState("summary", "task", taskId);
  useEffect(() => { live.current = true; return () => { live.current = false; request.current++; }; }, [taskId]);
  const refresh = useCallback(() => {
    const seq = ++request.current;
    api.getWorkstreamContextState(taskId).then(next => {
      if (live.current && seq === request.current) { setView(next); }
    }).catch(e => { if (live.current && seq === request.current) setError(contextUpdateErrorDetails(e)); });
  }, [taskId]);
  useEffect(refresh, [refresh, latestSeq]);
  useRefreshSignal(refresh);
  const update = async (regenerate: boolean) => {
    if (busyRef.current || operation.busy || operation.checking) return;
    busyRef.current = true; setBusy(regenerate ? "regenerate" : "update"); setError(null);
    try {
      const out = await (regenerate ? api.regenerateWorkstreamContext(taskId) : api.updateWorkstreamContext(taskId));
      if (!live.current) return;
      showToast(out.status === "no_change" ? "摘要无变化" : out.status === "skipped" ? "没有新的摘要内容" : "已更新协作摘要");
      refresh();
    } catch (e) { if (live.current) setError(contextUpdateErrorDetails(e)); }
    finally { await refreshOperationStatus(true); busyRef.current = false; if (live.current) setBusy(null); }
  };
  const disabled = !view || busy !== null || operation.busy || operation.checking;
  return <DetailSection sectionId="summary" label="协作摘要" className="collaboration-summary" title={<>
      <Icon name="spark" /><span>协作摘要{view?.summary ? `（${view.summary.items.length}）` : ""}</span>

    </>} actions={<>
      <SummaryControls expansion={expansion} />
      {view?.summary && <button type="button" className="btn ghost icon-button small" title="复制摘要" aria-label="复制摘要"
        onClick={async () => showToast((await copyToClipboard(summaryMarkdown(view.summary!))) ? "已复制摘要" : "复制失败")}><Icon name="copy" /></button>}
      <button type="button" className="btn ghost icon-button small" title="重新生成" aria-label="重新生成"
        disabled={disabled || !view || (view.processed_through_seq === 0 && view.pending_messages === 0)} onClick={() => void update(true)}>
        <Icon name="refresh" className={busy === "regenerate" ? "spinning" : ""} /></button>
      {view?.pending && <button type="button" className="btn primary icon-button small" title="更新摘要" aria-label="更新摘要" disabled={disabled} onClick={() => void update(false)}>
        <Icon name="spark" className={busy === "update" || operation.busy ? "spinning" : ""} /></button>}
    </>} defaultExpanded={!initiallyCollapsed} status={<>
      {view?.pending_messages ? <p className="muted small" role="status">有 {view.pending_messages} 条协作消息待整理</p> : null}
      {error && <div role="alert"><p>{error.message}</p><button type="button" className="btn small ghost" onClick={() => { setError(null); refresh(); }}>重试读取</button><button type="button" className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(error)); }}>复制错误详情</button></div>}
    </>}>


      {view?.summary ? <DetailContent label="协作摘要内容" className="md-body"><SummaryContent expansion={expansion}>{view.summary}</SummaryContent></DetailContent>
        : <p className="detail-empty">{view ? "尚无协作摘要；有协作消息后可提取。" : "正在读取摘要…"}</p>}
      {view?.updated_at && <p className="muted small" title={view.updated_at}>更新于 {timeAgo(view.updated_at)}</p>}

  </DetailSection>;
}
