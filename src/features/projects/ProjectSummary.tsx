import { DetailContent, DetailLoading, DetailSection } from "../../components/DetailSection";
import { useOperationState, refreshOperationStatus } from "../../hooks/useOperationState";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "../sessions/SummaryContent";
import type { ProjectContextView, SessionListItem } from "../../types";

import Icon from "../../components/Icon";

import { projectSummaryCache as cache, clearProjectSummaryCache } from "./projectSummaryCache";
export { clearProjectSummaryCache };


/** Pure reads on navigation; extraction requires an explicit user action. */
export default function ProjectSummary({ projectId, sessions }: { projectId: string; sessions: SessionListItem[] }) {
  const [view, setView] = useState<ProjectContextView | null>(() => cache.get(projectId) ?? null);
  const summaryExpansion = useSummaryExpansion(view?.summary);
  const operation = useOperationState("summary", "project", projectId);
  const progress = operation.projectSummaryProgress;
  const [busy, setBusy] = useState<"update" | "regenerate" | false>(false);
  const busyRef = useRef(false);
  const active = useRef(true);
  const request = useRef(0);
  const wasRunning = useRef(false);
  const [readError, setReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [error, setError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [failed, setFailed] = useState<{ session_id: string; error: string }[]>([]);



  const refresh = useCallback(() => {
    const seq = ++request.current;
    api.getProjectContextState(projectId).then(next => {
      if (!active.current || request.current !== seq) return;
      cache.set(projectId, next);
      setView(next);
      setReadError(null);
    }).catch(e => {
      if (active.current && request.current === seq) setReadError(contextUpdateErrorDetails(e));
    });
  }, [projectId]);
  useEffect(() => {
    active.current = true;
    refresh();
    return () => { active.current = false; ++request.current; };
  }, [refresh]);
  useRefreshSignal(refresh);
  useEffect(() => {
    if (wasRunning.current && !operation.busy && !busyRef.current) refresh();
    wasRunning.current = operation.busy;
  }, [operation.busy, refresh]);
  useEffect(() => {
    if (progress?.summary_saved) refresh();
  }, [progress?.summary_saved, refresh]);

  const update = async (regenerate = false) => {
    if (busyRef.current || operation.busy || operation.checking) return;
    busyRef.current = true;
    setBusy(regenerate ? "regenerate" : "update");
    setError(null);
    setFailed([]);
    try {
      const result = await (regenerate ? api.regenerateProjectContext(projectId) : api.updateProjectContext(projectId));
      if (!active.current) return;
      setFailed(result.failed_sessions);
      if (result.status === "partial") {
        const saved = result.updated_sessions.length + result.skipped_session_ids.length;
        const remaining = result.remaining_session_summaries;
        showToast(remaining > 0
          ? `已处理 ${saved} 个会话，剩余 ${remaining} 个`
          : "会话摘要已处理，项目摘要待汇总");
      } else {
        showToast(result.status === "no_change" ? "摘要无变化" : result.status === "skipped" ? "没有新的摘要内容" : result.remaining_pending > 0 ? `已更新摘要（待更新 ${result.remaining_pending}）` : "已更新摘要");
      }
      refresh();
    } catch (e) {
      if (active.current) setError(contextUpdateErrorDetails(e));
    } finally {
      await refreshOperationStatus(true);
      busyRef.current = false;
      if (active.current) setBusy(false);
    }
  };

  const shownError = error ?? readError;
  return (
    <DetailSection sectionId="summary" label="项目摘要" className="project-summary-section" title={<>
          <Icon name="spark" />
          <span>项目摘要{view?.summary ? `（${view?.summary.items.length}）` : ""}</span>
        </>} actions={<>
          <SummaryControls expansion={summaryExpansion} />
          {view?.summary && (
            <button
              type="button"
              className="btn ghost icon-button small"
              title="复制摘要"
              aria-label="复制摘要"
              onClick={async () => showToast((await copyToClipboard(summaryMarkdown(view.summary!))) ? "已复制摘要" : "复制失败")}
            >
              <Icon name="copy" />
            </button>
          )}
          <button
            type="button"
            className="btn ghost icon-button small"
            disabled={operation.busy || operation.checking || Boolean(busy) || !view}
            title={operation.error ? "无法读取更新状态" : busy === "regenerate" || operation.busy ? "重新生成中…" : "重新生成"}
            aria-label="重新生成"
            onClick={() => update(true)}
          >
            <Icon name="refresh" className={busy === "regenerate" || (operation.busy && !busy) ? "spinning" : ""} />
          </button>
          {view?.pending && (
            <button
              type="button"
              className="btn primary icon-button small"
              disabled={operation.busy || operation.checking || Boolean(busy)}
              title={operation.error ? "无法读取更新状态" : busy === "update" || operation.busy ? "更新中…" : "更新摘要"}
              aria-label="更新摘要"
              onClick={() => update()}
            >
              <Icon name="spark" className={busy === "update" || operation.busy ? "spinning" : ""} />
            </button>
          )}
        </>} status={<>
      {progress ? <p className="muted small" style={{ margin: 0 }} role="status">
        {progress.summary_saved ? "项目摘要已保存" : progress.remaining > 0
          ? `已处理 ${progress.processed} 个会话，剩余 ${progress.remaining} 个`
          : "正在汇总项目摘要"}
      </p> : view?.pending && <p className="muted small" style={{ margin: 0 }}>
        {view.pending_session_summaries > 0
          ? `${view.pending_session_summaries} 个会话待提取，完成后汇总项目摘要`
          : "项目摘要待汇总"}
      </p>}
      {failed.length > 0 && <div role="alert"><p>以下会话待重试</p><ul>{failed.map(f => <li key={f.session_id}>{sessions.find(s => s.id === f.session_id)?.title || f.session_id}：{f.error}</li>)}</ul></div>}
      {shownError && <div role="alert"><p>{shownError.message}</p><button className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(shownError)); }}>复制错误详情</button>{!view && <button className="btn small ghost" onClick={() => { setReadError(null); refresh(); }}>重试</button>}</div>}
    </>}>


      {view && view.summary ? <DetailContent label="项目摘要内容" className="md-body"><SummaryContent expansion={summaryExpansion}
       >{view.summary ?? {items:[]}}</SummaryContent></DetailContent> : view ? <p className="detail-empty">尚无项目摘要</p>
       : !shownError && <DetailLoading>正在读取摘要…</DetailLoading>}


    </DetailSection>
  );
}
