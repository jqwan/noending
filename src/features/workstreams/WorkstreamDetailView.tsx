import { useSummaryEditor } from "../sessions/useSummaryEditor";
import { removedTargets } from "../../app/removedTargets";
import { useCallback, useEffect, useRef, useState } from "react";
import "./workstreams.css";
import { api } from "../../api";

import PageHeader from "../../layout/PageHeader";
import Icon from "../../components/Icon";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import WorkstreamFormModal from "./WorkstreamFormModal";
import WorkstreamProjectPickerModal from "./WorkstreamProjectPickerModal";
import {
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamContextView,
} from "../../types";
import type { Route } from "../../app/routes";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "../sessions/SummaryContent";
import WorkstreamSessions from "./WorkstreamSessions";

/** Task organization and the current Markdown summary. All reads are pure. */
import { workstreamDetailCache, clearWorkstreamDetailCache } from "./workstreamDetailCache";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
export { workstreamDetailCache, clearWorkstreamDetailCache };


export default function WorkstreamDetailView({
  workstreamId,
  initialTitle,
  navigate,
  goBack,
}: {
  workstreamId: string;
  initialTitle?: string;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const currentWorkstreamId = useRef(workstreamId);
  currentWorkstreamId.current = workstreamId;
  const cached = workstreamDetailCache.get(workstreamId);
  const [loadError, setLoadError] = useState("");
  const [retry, setRetry] = useState(0);
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(() => cached?.ctx ?? null);
  const showSkeleton = useLoadingDelay(!ctx && loadError === "");
  /** Current items, revision and extraction coverage. */
  const [ctxState, setCtxState] = useState<WorkstreamContextView | null>(() => cached?.ctxState ?? null);
  const summaryExpansion = useSummaryExpansion(ctxState?.summary);
  const [failedSessions, setFailedSessions] = useState<{ session_id: string; error: string }[]>([]);
  const [ctxUpdating, setCtxUpdating] = useState<"update" | "regenerate" | false>(false);
  const [ctxUpdateError, setCtxUpdateError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [actionError, setActionError] = useState("");
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const busyRef = useRef(false);

  const summaryEditor = useSummaryEditor({
    target: ctxState?.workstream_id === workstreamId ? { kind: "task", id: workstreamId, revision: ctxState.context_revision } : undefined,
    summary: ctxState?.summary,
    disabled: Boolean(ctxUpdating),
    onChange: (result, removedIndex) => {
      summaryExpansion.preserve(result.summary, removedIndex);
      if (currentWorkstreamId.current !== workstreamId) return;
      setCtxState(previous => {
        if (!previous || previous.context_revision > result.revision) return previous;
        const next = {...previous,context_revision:result.revision,summary:result.summary.items.length ? result.summary : null};
        const cached = workstreamDetailCache.get(workstreamId);
        if (cached) workstreamDetailCache.set(workstreamId,{...cached,ctxState:next});
        return next;
      });
    },
  });

  useEffect(() => {
    let cancelled = false;
    const currentCached = workstreamDetailCache.get(workstreamId);
    if (currentCached) {
      setCtx(currentCached.ctx);
      setCtxState(currentCached.ctxState);
    } else {
      setCtx(null);
      setCtxState(null);
    }
    setLoadError("");
    setCtxUpdateError(null);
    setFailedSessions([]);
    setCtxUpdating(false);
    setEditing(false);
    setConfirmArchive(false);
    setConfirmPurge(false);
    setActionError("");

    api.getWorkstreamContext(workstreamId).then((c) => {
      if (cancelled || removedTargets.workstreams.has(workstreamId)) return;
      setCtx(c);
      const prev = workstreamDetailCache.get(workstreamId);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
        ctxState: prev?.ctxState ?? null,
      });
    }).catch(e => { if (!cancelled) setLoadError(String(e)); });

    api.getWorkstreamContextState(workstreamId).then((s) => {
      if (cancelled || removedTargets.workstreams.has(workstreamId)) return;
      setCtxState(s);
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctxState: s });
      }
    }).catch(e => { if (!cancelled) setCtxUpdateError(contextUpdateErrorDetails(e)); });

    return () => {
      cancelled = true;
    };
  }, [workstreamId, retry]);

  // Background refresh only reads task data and summary state.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (currentWorkstreamId.current !== workstreamId || removedTargets.workstreams.has(workstreamId)) return;
      setCtx(c);
      const prev = workstreamDetailCache.get(workstreamId);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
        ctxState: prev?.ctxState ?? null,
      });
    }).catch(console.error);

    api.getWorkstreamContextState(workstreamId).then((s) => {
      if (currentWorkstreamId.current !== workstreamId || removedTargets.workstreams.has(workstreamId)) return;
      setCtxState(s);
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctxState: s });
      }
    }).catch(console.error);


  }, [workstreamId]);

  useRefreshSignal(refresh);

  /** 一次点击 → 相关 Session 摘要与 Workstream 状态一起更新（后端一次模型调用）。 */
  const updateContext = async (regenerate = false) => {
    if (ctxUpdating || summaryEditor.editing) return;
    setCtxUpdating(regenerate ? "regenerate" : "update");
    setCtxUpdateError(null);
    setFailedSessions([]);
    try {
      const out = await (regenerate ? api.regenerateWorkstreamContext(workstreamId) : api.updateWorkstreamContext(workstreamId));
      if (currentWorkstreamId.current !== workstreamId || removedTargets.workstreams.has(workstreamId)) return;
      setFailedSessions(out.failed_sessions);
      showToast(
        out.status === "no_change"
          ? "摘要无变化"
          : out.remaining_pending > 0
            ? `已更新摘要（待更新 ${out.remaining_pending}）`
            : out.status === "skipped" ? "没有新的摘要内容" : "已更新摘要",
      );
      refresh();
    } catch (e) {
      console.error(e);
      if (currentWorkstreamId.current === workstreamId) setCtxUpdateError(contextUpdateErrorDetails(e));
    } finally {
      if (currentWorkstreamId.current === workstreamId) setCtxUpdating(false);
    }
  };

  /** Copy the current summary as Markdown. */
  const copyContext = async () => {
    const ok = await copyToClipboard(ctxState?.summary ? summaryMarkdown(ctxState.summary) : "");
    showToast(ok ? "已复制摘要" : "复制失败");
  };

  // 同 SessionDetailView：加载态也要渲染 PageHeader，否则整条标题栏会先消失再补回来。
  if (!ctx) {
    if (loadError === "") {
      return (
        <div className="main task-detail" role="status">
          <PageHeader title={initialTitle ?? ""} />
          {showSkeleton && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
              <div className="skeleton" style={{ height: 120, borderRadius: "var(--radius-lg)" }} />
              <div className="skeleton" style={{ height: 200, borderRadius: "var(--radius-lg)" }} />
            </div>
          )}
        </div>
      );
    }
    return (
      <div className="main narrow" role="status">
        <PageHeader title={initialTitle ?? ""}>
          <p>{loadError}</p>
          <button className="btn" onClick={() => setRetry(value => value + 1)}>重试</button>
        </PageHeader>
      </div>
    );
  }
  const { workstream, sessions } = ctx;

  /** 一条命令返回新 Workstream 时立刻就地替换：`update_workstream` 是整对象写，
   *  留着旧的 visibility，下一次整对象保存会被后端拒绝。 */
  const adopt = (next: Workstream) => setCtx((c) => {
    const nextCtx = c ? { ...c, workstream: next } : c;
    if (nextCtx) {
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctx: nextCtx });
      }
    }
    return nextCtx;
  });

  /**
   * 归档与恢复是两个单向命令：`archive_workstream` 只进已归档，
   * `restore_workstream` 只出来——不是一枚翻转开关（那会造出双权威）。
   * 失败留在原地说明原因，成功才跳走。
   */
  const archiveTask = async () => {
    setConfirmArchive(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.archiveWorkstream(workstream.id));
      goBack({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`归档失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const restore = async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.restoreWorkstream(workstream.id));
      refresh();
    } catch (e) {
      console.error(e);
      setActionError(`恢复失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /** 唯一不可逆的动作，且后端只接受从已归档出发。 */
  const purge = async () => {
    setConfirmPurge(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      await api.deleteWorkstreamPermanently(workstream.id);
      workstreamDetailCache.delete(workstream.id);
      if (mounted.current) navigate({ view: "workstreams", scope: "archived" });
    } catch (e) {
      console.error(e);
      setActionError(`永久删除失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const openEditor = () => {
    setEditing(true);
  };

  const archived = workstream.visibility === "archived";
  // 一个 Workstream 可以关联多个 Project
  const projectRows=ctx.projects;

  return (
    <div className="main task-detail">
      <PageHeader
        title={
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8, overflowWrap: "anywhere" }}>
            <Icon name="tasks" />
            <span>{workstream.title}</span>
          </span>
        }
        actions={
          <>
            <button className="btn ghost icon-button" title="编辑任务" aria-label="编辑任务"
              onClick={openEditor}>
              <Icon name="edit" />
            </button>
            {!archived ? (
              <button className="btn ghost icon-button" aria-label="归档"
                title="归档：保留项目关联、会话跟踪与摘要，归档后不能新建会话。"
                onClick={() => setConfirmArchive(true)}>
                <Icon name="archive" />
              </button>
            ) : (
              <>
                <button
                  className="btn ghost icon-button"
                  title="取消归档"
                  aria-label="取消归档"
                  disabled={busy}
                  onClick={restore}
                >
                  <Icon name="unarchive" />
                </button>
                <button
                  className="btn ghost icon-button danger"
                  title="永久删除"
                  aria-label="永久删除"
                  disabled={busy}
                  onClick={() => setConfirmPurge(true)}
                >
                  <Icon name="trash" />
                </button>
              </>
            )}
          </>
        }
      >
        {actionError && (
          <div className="small" style={{ color: "var(--warning)", marginTop: 6, overflowWrap: "anywhere" }}>
            {actionError}
          </div>
        )}
      </PageHeader>

      {/* Workstream 自身（Base Experience）：与另外两个详情页同形——主栏是内容本身
          （概览、会话、Context），右栏是只读事实（状态、路径、项目）。 */}
      <div className="detail-layout task-detail-layout">
        <div className="detail-main task-detail-main">
          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <h2 className="detail-section-title">
                <Icon name="tasks" />
                <span>任务概览</span>
              </h2>
            </div>
            {workstream.description ? (
              <p style={{ margin: 0, maxWidth: "72ch", overflowWrap: "anywhere" }}>
                {workstream.description}
              </p>
            ) : (
              <div className="l1-none">还没有描述。</div>
            )}
          </section>

          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <h2 className="detail-section-title">
                <Icon name="spark" />
                <span>任务摘要{ctxState?.summary ? `（${ctxState?.summary.items.length}）` : ""}</span>
              </h2>
              <div className="actions" style={{ display: "flex", gap: 8 }}>
                <SummaryControls expansion={summaryExpansion} editor={summaryEditor} />
                {ctxState?.summary && (
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
                <button
                  type="button"
                  className="btn ghost icon-button small"
                  disabled={Boolean(ctxUpdating) || summaryEditor.editing}
                  title={ctxUpdating === "regenerate" ? "重新生成中…" : "重新生成"}
                  aria-label="重新生成"
                  onClick={() => updateContext(true)}
                >
                  <Icon name="refresh" className={ctxUpdating === "regenerate" ? "spinning" : ""} />
                </button>
                {ctxState?.pending && (
                  <button
                    type="button"
                    className="btn primary icon-button small"
                    disabled={Boolean(ctxUpdating) || summaryEditor.editing}
                    title={ctxUpdating === "update" ? "更新中…" : "更新摘要"}
                    aria-label="更新摘要"
                    onClick={() => updateContext()}
                  >
                    <Icon name="spark" className={ctxUpdating === "update" ? "spinning" : ""} />
                  </button>
                )}
              </div>
            </div>
            {ctxState?.pending && <p className="muted small" style={{ margin: 0 }}>有待更新内容{ctxState.pending_sessions > 0 ? `，涉及 ${ctxState.pending_sessions} 个会话` : ""}</p>}
            {ctxState && (ctxState.summary || summaryEditor.editing) ? <div className="md-body" style={{ overflowWrap: "anywhere" }}><SummaryContent expansion={summaryExpansion}
              editor={summaryEditor}>{ctxState.summary ?? {items:[]}}</SummaryContent></div> : <p className="muted small" style={{ margin: 0 }}>尚无任务摘要</p>}
            {failedSessions.length > 0 && <div role="alert"><p>任务摘要已更新，以下会话仍待重试：</p><ul>{failedSessions.map(f => <li key={f.session_id}>{sessions.find(s => s.id === f.session_id)?.title || f.session_id}：{f.error}</li>)}</ul></div>}
            {ctxUpdateError && <div role="alert"><p>{ctxUpdateError.message}</p><button className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(ctxUpdateError)); }}>复制错误详情</button></div>}
          </section>
        </div>

        <aside className="detail-aside task-detail-aside">
          <section className="card detail-section">
            <h2 className="detail-section-title">
              <Icon name="info" />
              <span>状态</span>
            </h2>
            <span className="badge" style={{ alignSelf: "flex-start" }}>{archived ? "已归档" : "未归档"}</span>
            <div className="small muted">
              创建于 {formatDate(workstream.created_at)} · 最近更新 {timeAgo(workstream.updated_at)}
            </div>
          </section>

          <WorkstreamSessions
            sessions={sessions}
            navigate={navigate}
            onNewSession={archived || projectRows.length === 0 ? undefined : () => navigate({ view: "new-session", workstreamId: workstream.id })}
            allowActions={!archived && projectRows.length > 0}
          />

          <section className="card detail-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <h2 className="detail-section-title">
                <Icon name="project" />
                <span>关联项目（{projectRows.length}）</span>
              </h2>
              {!archived && (
                <button
                  type="button"
                  className="btn ghost icon-button small"
                  title="关联项目"
                  aria-label="关联项目"
                  onClick={() => setProjectPickerOpen(true)}
                >
                  <Icon name="plus" />
                </button>
              )}
            </div>
            {projectRows.length === 0 && (
              <div className="l1-none">
                暂无项目
              </div>
            )}
            {projectRows.map((p) => (
              <div className="list-row" key={p.id} role="link" tabIndex={0}
                onKeyDown={(e) => { if (e.key === "Enter") navigate({ view: "project", projectId: p.id, initialTitle: p.name ?? undefined }); }}
                onClick={() => navigate({ view: "project", projectId: p.id, initialTitle: p.name ?? undefined })}>
                <div className="grow">
                  <div className="title" title={p.name ?? p.id} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                    <Icon name="project" />
                    <span>{p.name ?? "未命名项目"}</span>
                  </div>
                </div>
              </div>
            ))}
          </section>
        </aside>
      </div>

      {projectPickerOpen && (
        <WorkstreamProjectPickerModal
          initialSelectedIds={projectRows.map((p) => p.id)}
          onClose={() => setProjectPickerOpen(false)}
          onConfirm={async (nextSelected) => {
            await api.setWorkstreamProjects(workstream.id, nextSelected);
            refresh();
          }}
        />
      )}

      {editing && (
        <WorkstreamFormModal
          workstream={workstream}
          projects={projectRows}
          onClose={() => setEditing(false)}
          onSaved={refresh}
        />
      )}

      {confirmArchive && (
        <Modal title="归档" onClose={() => setConfirmArchive(false)}>
          <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
            <b>{workstream.title}</b> 会移至任务页的「已归档」，归档后不能用于新建会话。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            路径、会话归属和上下文都会保留，可取消归档。
          </p>

          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmArchive(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={archiveTask} disabled={busy}>
              {busy ? "处理中…" : "归档"}
            </button>
          </div>
        </Modal>
      )}

      {confirmPurge && (
        <Modal title="永久删除这项任务？" onClose={() => { if (!busy) setConfirmPurge(false); }}>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>不可撤销</div>
          <p style={{ margin: "0 0 12px", maxWidth: "72ch" }}>
            确定要永久删除 <b>{workstream.title}</b> 吗？任务本身、工作路径列表、Context 和审阅数据会被删除，关联会话及其事件历史会保留。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmPurge(false)} disabled={busy}>取消</button>
            <button className="btn danger" onClick={purge} disabled={busy}>
              {busy ? "删除中…" : "永久删除"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/**
 * RFC3339 (UTC) → 本地 YYYY/MM/DD。不用 `toLocaleDateString()`（webview 语言不固定，
 * 格式会漂移），也不用 `slice(0, 10)`（那是 UTC 日期，本地可能已跨天）。
 */
function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getFullYear()}/${pad(t.getMonth() + 1)}/${pad(t.getDate())}`;
}
