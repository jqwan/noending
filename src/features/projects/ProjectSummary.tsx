import { useSummaryEditor } from "../sessions/useSummaryEditor";
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
  const [busy, setBusy] = useState<"update" | "regenerate" | false>(false);
  const busyRef = useRef(false);
  const active = useRef(true);
  const request = useRef(0);
  const [readError, setReadError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [error, setError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [failed, setFailed] = useState<{ session_id: string; error: string }[]>([]);

  const summaryEditor = useSummaryEditor({
    target: view?.project_id === projectId ? { kind: "project", id: projectId, revision: view.context_revision } : undefined,
    summary: view?.summary,
    disabled: Boolean(busy),
    onChange: (result, removedIndex) => {
      summaryExpansion.preserve(result.summary, removedIndex);
      setView(previous => {
        if (!previous || previous.context_revision > result.revision) return previous;
        const next = {...previous,context_revision:result.revision,summary:result.summary.items.length ? result.summary : null};
        cache.set(projectId,next);
        return next;
      });
    },
  });

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

  const update = async (regenerate = false) => {
    if (busyRef.current || summaryEditor.editing) return;
    busyRef.current = true;
    setBusy(regenerate ? "regenerate" : "update");
    setError(null);
    setFailed([]);
    try {
      const result = await (regenerate ? api.regenerateProjectContext(projectId) : api.updateProjectContext(projectId));
      if (!active.current) return;
      setFailed(result.failed_sessions);
      showToast(result.status === "no_change" ? "摘要无变化" : result.status === "skipped" ? "没有新的摘要内容" : result.remaining_pending > 0 ? `已更新摘要（待更新 ${result.remaining_pending}）` : "已更新摘要");
      refresh();
    } catch (e) {
      if (active.current) setError(contextUpdateErrorDetails(e));
    } finally {
      busyRef.current = false;
      if (active.current) setBusy(false);
    }
  };

  const shownError = error ?? readError;
  return (
    <section className="card detail-section project-summary-section">
      <div className="row between" style={{ alignItems: "center" }}>
        <h2 className="detail-section-title">
          <Icon name="spark" />
          <span>项目摘要{view?.summary ? `（${view?.summary.items.length}）` : ""}</span>
        </h2>
        <div className="actions" style={{ display: "flex", gap: 8 }}>
          <SummaryControls expansion={summaryExpansion} editor={summaryEditor} />
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
            disabled={Boolean(busy) || !view || summaryEditor.editing}
            title={busy === "regenerate" ? "重新生成中…" : "重新生成"}
            aria-label="重新生成"
            onClick={() => update(true)}
          >
            <Icon name="refresh" className={busy === "regenerate" ? "spinning" : ""} />
          </button>
          {view?.pending && (
            <button
              type="button"
              className="btn primary icon-button small"
              disabled={Boolean(busy) || summaryEditor.editing}
              title={busy === "update" ? "更新中…" : "更新摘要"}
              aria-label="更新摘要"
              onClick={() => update()}
            >
              <Icon name="spark" className={busy === "update" ? "spinning" : ""} />
            </button>
          )}
        </div>
      </div>
      {view?.pending && <p className="muted small" style={{ margin: 0 }}>有待更新内容{view.pending_sessions > 0 ? `，涉及 ${view.pending_sessions} 个会话` : ""}</p>}
      {view && (view.summary || summaryEditor.editing) ? <div className="md-body" style={{ overflowWrap: "anywhere" }}><SummaryContent expansion={summaryExpansion}
        editor={summaryEditor}>{view.summary ?? {items:[]}}</SummaryContent></div> : <p className="muted small" style={{ margin: 0 }}>{view ? "尚无项目摘要" : "正在读取摘要…"}</p>}
      {failed.length > 0 && <div role="alert"><p>项目摘要已更新，以下会话仍待重试：</p><ul>{failed.map(f => <li key={f.session_id}>{sessions.find(s => s.id === f.session_id)?.title || f.session_id}：{f.error}</li>)}</ul></div>}
      {shownError && <div role="alert"><p>{shownError.message}</p><button className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(shownError)); }}>复制错误详情</button>{!view && <button className="btn small ghost" onClick={() => { setReadError(null); refresh(); }}>重试</button>}</div>}
    </section>
  );
}
