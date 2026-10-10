import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { MD_COMPONENTS } from "../sessions/SessionMessage";
import type { ProjectContextView, Session } from "../../types";

const cache = new Map<string, ProjectContextView>();
export function clearProjectSummaryCache() { cache.clear(); }

/** Pure reads on navigation; extraction requires an explicit user action. */
export default function ProjectSummary({ projectId, sessions }: { projectId: string; sessions: Session[] }) {
  const [view, setView] = useState<ProjectContextView | null>(() => cache.get(projectId) ?? null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const active = useRef(true);
  const request = useRef(0);
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

  const update = async (regenerate = false) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
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
  return <section className="rail-section">
    <div className="rail-head">
      <div className="section-label" style={{ margin: 0 }}>项目摘要</div>
      <div className="actions">
        {view?.summary && <button className="btn small ghost" onClick={async () => showToast(await copyToClipboard(view.summary!) ? "已复制摘要" : "复制失败")}>复制</button>}
        {view?.pending && <button className="btn small primary" disabled={busy} onClick={() => update()}>{busy ? "更新中…" : "更新摘要"}</button>}
        <button className="btn small ghost" disabled={busy || !view} onClick={() => update(true)}>重新生成</button>
      </div>
    </div>
    {view?.pending && <p className="muted">有待更新内容{view.pending_sessions > 0 ? `，涉及 ${view.pending_sessions} 个会话` : ""}</p>}
    {view?.summary ? <div className="md-body" style={{ maxWidth: "72ch", overflowWrap: "anywhere" }}><ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{view.summary}</ReactMarkdown></div> : <p className="muted">{view ? "尚无项目摘要" : "正在读取摘要…"}</p>}
    {failed.length > 0 && <div role="alert"><p>项目摘要已更新，以下会话仍待重试：</p><ul>{failed.map(f => <li key={f.session_id}>{sessions.find(s => s.id === f.session_id)?.title || f.session_id}：{f.error}</li>)}</ul></div>}
    {shownError && <div role="alert"><p>{shownError.message}</p><button className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(shownError)); }}>复制错误详情</button>{!view && <button className="btn small ghost" onClick={() => { setReadError(null); refresh(); }}>重试</button>}</div>}
  </section>;
}
