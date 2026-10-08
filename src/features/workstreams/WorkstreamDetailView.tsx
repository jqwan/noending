import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import Icon from "../../components/Icon";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, submitsOnEnter, timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import WorkstreamFormModal from "./WorkstreamFormModal";
import {
  KIND_LABELS,
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamContextView,
  type WorkstreamPathRow,
  type WorkstreamReviewSummary,
  type WorkstreamReviewWindow,
} from "../../types";
import type { Route, WorkstreamEntry } from "../../app/routes";
import WorkstreamContext from "./WorkstreamContext";
import WorkstreamSessions from "./WorkstreamSessions";
import WorkstreamPathList from "./WorkspacePaths";
import NeedsAttentionSection from "./NeedsAttentionSection";
import ConflictReviewModal from "./ConflictReviewModal";
import RecentChangesTimeline from "./RecentChangesTimeline";
import SinceLastReview from "./SinceLastReview";

/**
 * Workstream Detail：持续相关 Sessions 的组织容器。
 *
 * Context 只读呈现（`get_workstream_context_state`），人工纠正永远可用；仅当有相关
 * 变化（pending）时才出现「更新状态」按钮，一次点击同时更新相关 Session 摘要与状态。
 * 启动 / 编辑入口唯一，分别挂 New Session Modal 与 WorkstreamFormModal。
 * v0.2 边界：工作目录是有序 WorkstreamPath 列表；Project 是只读投影（取第 1 条路径）；
 * 归档 / 恢复 / 永久删除是三个单向命令，不是一枚翻转开关。
 */
export const workstreamDetailCache = new Map<
  string,
  {
    ctx: WorkstreamContextData;
    ctxState: WorkstreamContextView | null;
    paths: WorkstreamPathRow[] | null;
  }
>();

export function clearWorkstreamDetailCache() {
  workstreamDetailCache.clear();
}

export default function WorkstreamDetailView({
  workstreamId,
  entry,
  navigate,
  goBack,
}: {
  workstreamId: string;
  entry?: WorkstreamEntry;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const cached = workstreamDetailCache.get(workstreamId);
  const [loadError, setLoadError] = useState("");
  const [retry, setRetry] = useState(0);
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(() => cached?.ctx ?? null);
  /** Context 状态（只读）：当前投影 + revision + 待更新。 */
  const [ctxState, setCtxState] = useState<WorkstreamContextView | null>(() => cached?.ctxState ?? null);
  const [ctxUpdating, setCtxUpdating] = useState(false);
  const [ctxUpdateError, setCtxUpdateError] = useState<ReturnType<typeof contextUpdateErrorDetails> | null>(null);
  const [paths, setPaths] = useState<WorkstreamPathRow[] | null>(() => cached?.paths ?? null);
  const [pathsError, setPathsError] = useState("");
  const [busy, setBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [actionError, setActionError] = useState("");
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const [purgeConfirmText, setPurgeConfirmText] = useState("");
  const busyRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // ••• 菜单：点击外部与 Escape 都要收起。mousedown 阶段监听先于 click，
  // 所以菜单项自己的 click 仍然正常触发。
  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    let cancelled = false;
    const currentCached = workstreamDetailCache.get(workstreamId);
    if (currentCached) {
      setCtx(currentCached.ctx);
      setCtxState(currentCached.ctxState);
      setPaths(currentCached.paths);
    } else {
      setCtx(null);
      setCtxState(null);
      setPaths(null);
    }
    setLoadError("");
    setCtxUpdateError(null);
    setPathsError("");
    setMenuOpen(false);
    setEditing(false);
    setConfirmArchive(false);
    setConfirmPurge(false);
    setActionError("");

    api.getWorkstreamContext(workstreamId).then((c) => {
      if (cancelled) return;
      setCtx(c);
      const prev = workstreamDetailCache.get(workstreamId);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
        ctxState: prev?.ctxState ?? null,
        paths: prev?.paths ?? null,
      });
    }).catch(e => { if (!cancelled) setLoadError(String(e)); });

    api.getWorkstreamContextState(workstreamId).then((s) => {
      if (cancelled) return;
      setCtxState(s);
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctxState: s });
      }
    }).catch(e => { if (!cancelled) setCtxUpdateError(contextUpdateErrorDetails(e)); });

    api.listWorkstreamPaths(workstreamId).then((rows) => {
      if (cancelled) return;
      setPaths(rows);
      setPathsError("");
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, paths: rows });
      }
    }).catch((e) => {
      if (!cancelled) { setPaths(null); setPathsError(`读取工作目录失败：${String(e)}`); }
    });
    return () => {
      cancelled = true;
    };
  }, [workstreamId, retry]);

  // Background refresh only re-reads the projection; the intelligence panels own
  // their own (frozen-window) refresh so this page never touches ReviewState.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then((c) => {
      setCtx(c);
      const prev = workstreamDetailCache.get(workstreamId);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
        ctxState: prev?.ctxState ?? null,
        paths: prev?.paths ?? null,
      });
    }).catch(console.error);

    api.getWorkstreamContextState(workstreamId).then((s) => {
      setCtxState(s);
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctxState: s });
      }
    }).catch(console.error);

    api.listWorkstreamPaths(workstreamId).then((rows) => {
      setPaths(rows);
      setPathsError("");
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, paths: rows });
      }
    }).catch((e) => {
      setPaths(null);
      setPathsError(`读取工作目录失败：${String(e)}`);
    });
  }, [workstreamId]);

  useRefreshSignal(refresh);

  /** 一次点击 → 相关 Session 摘要与 Workstream 状态一起更新（后端一次模型调用）。 */
  const updateContext = async () => {
    if (ctxUpdating) return;
    setCtxUpdating(true);
    setCtxUpdateError(null);
    try {
      const out = await api.updateWorkstreamContext(workstreamId);
      showToast(
        out.status === "no_change"
          ? "状态无变化"
          : out.remaining_pending > 0
            ? `已更新状态（待更新 ${out.remaining_pending}）`
            : "已更新状态",
      );
      refresh();
    } catch (e) {
      console.error(e);
      setCtxUpdateError(contextUpdateErrorDetails(e));
    } finally {
      setCtxUpdating(false);
    }
  };

  /** 复制当前 Context：投影按可读文本整段给出去。 */
  const copyContext = async () => {
    const sections = ctxState?.sections ?? [];
    const text = sections.length === 0
      ? "（当前没有 Context 条目）"
      : sections
        .map((s) => `${KIND_LABELS[s.kind] ?? s.kind}：${s.title}\n${s.content}`.trim())
        .join("\n\n");
    const ok = await copyToClipboard(text);
    showToast(ok ? "已复制 Context" : "复制失败");
  };

  // 同 SessionDetailView：加载态也要渲染 PageHeader，否则整条标题栏会先消失再补回来。
  if (!ctx) {
    if (loadError === "") {
      return (
        <div className="main task-detail" role="status">
          <PageHeader title={<span className="skeleton" style={{ display: "inline-block", width: 180, height: 24, borderRadius: "var(--radius-sm)" }} />} />
          <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
            <div className="skeleton" style={{ height: 120, borderRadius: "var(--radius-lg)" }} />
            <div className="skeleton" style={{ height: 200, borderRadius: "var(--radius-lg)" }} />
          </div>
        </div>
      );
    }
    return (
      <div className="main narrow" role="status">
        <PageHeader title="读取任务失败">
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
    setMenuOpen(false);
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
    setMenuOpen(false);
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
      goBack({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`永久删除失败：${String(e)}`);
      setPurgeConfirmText("");
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /** 工作目录还没读回来时不能进编辑模式：草稿由当前列表预填，拿「空」当
   *  「没有目录」会在保存时把已有路径全删掉，所以明说还在读。 */
  const openEditor = () => {
    setMenuOpen(false);
    if (paths === null) {
      setActionError("工作目录还在读取中，请稍后再试。");
      return;
    }
    setEditing(true);
  };

  const archived = workstream.visibility === "archived";
  // 一个 Workstream 可以关联多个 Project
  const projectRows = (() => {
    const byId = new Map<string, { id: string; name: string | null; count: number }>();
    for (const p of paths ?? []) {
      const seen = byId.get(p.project_id);
      byId.set(p.project_id, {
        id: p.project_id,
        name: p.project_name ?? seen?.name ?? null,
        count: (seen?.count ?? 0) + 1,
      });
    }
    return [...byId.values()];
  })();

  return (
    <div className="main task-detail">
      <PageHeader
        title={<span style={{ overflowWrap: "anywhere" }}>{workstream.title}</span>}
        actions={
          <>
            <button className="btn ghost icon-button" title="编辑任务" aria-label="编辑任务"
              onClick={openEditor}>
              <Icon name="edit" />
            </button>
            {!archived && (
              <button className="btn ghost icon-button" aria-label="归档"
                title="归档：保留路径、会话与 Context，归档后不能新建会话。"
                onClick={() => setConfirmArchive(true)}>
                <Icon name="archive" />
              </button>
            )}
            {archived && (
              <div style={{ position: "relative" }} ref={menuRef}>
                <button className="btn ghost icon-button" onClick={() => setMenuOpen((v) => !v)} title="更多操作"
                  aria-label="更多操作" aria-haspopup="true" aria-expanded={menuOpen}>
                  <Icon name="more" />
                </button>
                {menuOpen && (
                  <div className="menu-pop">
                    <button className="menu-item" onClick={restore}>
                      取消归档
                    </button>
                    <button className="menu-item"
                      title="永久删除（不可撤销）"
                      onClick={() => { setMenuOpen(false); setPurgeConfirmText(""); setConfirmPurge(true); }}>
                      永久删除…
                    </button>
                  </div>
                )}
              </div>
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
      <div className="task-detail-layout">
        <div className="task-detail-main">
        <section className="rail-section">
          <div className="rail-head">
            <div className="section-label" style={{ margin: 0 }}>任务概览</div>
          </div>
          {workstream.description ? (
            <p style={{ margin: 0, maxWidth: "72ch", overflowWrap: "anywhere" }}>
              {workstream.description}
            </p>
          ) : (
            <div className="l1-none">还没有描述。</div>
          )}
        </section>

        <WorkstreamSessions
          sessions={sessions}
          navigate={navigate}
          onNewSession={archived ? undefined : () => navigate({ view: "new-session", workstreamId: workstream.id })}
          allowActions={!archived}
        />

        {/* Context：只读当前状态 + 显式更新 + 人工纠正 */}
        <IntelligenceSections
          ctx={ctx}
          entry={entry}
          workstreamId={workstreamId}
          navigate={navigate}
          onChanged={refresh}
          ctxState={ctxState}
          ctxUpdating={ctxUpdating}
          ctxUpdateError={ctxUpdateError}
          onCopyContext={copyContext}
          onUpdateContext={updateContext}
        />

        </div>
        <aside className="task-detail-aside">
        <section className="rail-section">
          <div className="section-label">状态</div>
          <span className="badge">{archived ? "已归档" : "未归档"}</span>
          <div className="small muted" style={{ marginTop: 6 }}>
            创建于 {formatDate(workstream.created_at)} · 最近更新 {timeAgo(workstream.updated_at)}
          </div>
        </section>

        <WorkstreamPathList
          paths={paths}
          error={pathsError}
        />

        <section className="rail-section">
          <div className="section-label">项目</div>
          {paths === null && (
            <div className="muted small">{pathsError || "读取工作目录后才能确定…"}</div>
          )}
          {paths !== null && projectRows.length === 0 && (
            <div className="l1-none">
              暂无项目
            </div>
          )}
          {paths !== null && projectRows.map((p) => (
            <div className="list-row" key={p.id} role="link" tabIndex={0}
              onKeyDown={(e) => { if (e.key === "Enter") navigate({ view: "project", projectId: p.id }); }}
              onClick={() => navigate({ view: "project", projectId: p.id })}>
              <div className="grow">
                <div className="title" title={p.name ?? p.id}>{p.name ?? "未命名项目"}</div>
              </div>
            </div>
          ))}

        </section>
        </aside>
      </div>

      {editing && paths !== null && (
        <WorkstreamFormModal
          workstream={workstream}
          paths={paths}
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
        <Modal title="永久删除这项任务？" onClose={() => setConfirmPurge(false)}>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            不可撤销
          </div>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将<b>删除</b>：<span className="mono">{workstream.title}</span> 本身、它的有序工作目录列表、
            它名下的全部 Context 条目与 Revision、冲突记录与解决历史、以及审阅状态（ReviewState）。
          </p>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将<b>保留</b>：它引用过的 Sessions 与这些 Session 的完整事件历史、原始 Agent
            会话文件、启动记录、以及工作目录本身（WorkspacePath）与由它派生的 Project。
            换句话说：Project 与 Session 都不会因为删掉一项任务而受影响。
          </p>
          <p className="small muted" style={{ marginBottom: 12 }}>
            只有已经在已归档的任务才能被永久删除 —— 这是刻意留的缓冲。
          </p>
          <label className="field"><span>输入这项任务的标题以确认</span>
            <input type="text" value={purgeConfirmText} autoFocus
              placeholder={workstream.title}
              onChange={(e) => setPurgeConfirmText(e.target.value)}
              onKeyDown={(e) => submitsOnEnter(e) && purgeConfirmText === workstream.title && purge()} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmPurge(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={purge}
              disabled={busy || purgeConfirmText !== workstream.title}>
              {busy ? "删除中…" : "确认永久删除"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/** Context 段落：当前状态只读呈现 + 显式「更新状态」，以及审查 / 冲突 /
 *  变更这些由本页命令驱动的面板（人工纠正始终可用）。 */
function IntelligenceSections({
  ctx,
  entry,
  workstreamId,
  navigate,
  onChanged,
  ctxState,
  ctxUpdating,
  ctxUpdateError,
  onCopyContext,
  onUpdateContext,
}: {
  ctx: WorkstreamContextData;
  entry?: WorkstreamEntry;
  workstreamId: string;
  navigate: (r: Route) => void;
  onChanged: () => void;
  ctxState: WorkstreamContextView | null;
  ctxUpdating: boolean;
  ctxUpdateError: ReturnType<typeof contextUpdateErrorDetails> | null;
  onCopyContext: () => void;
  onUpdateContext: () => void;
}) {
  const [reviewWindow, setReviewWindow] = useState<WorkstreamReviewWindow | null>(null);
  const [reviewSummary, setReviewSummary] = useState<WorkstreamReviewSummary | null>(null);
  const [reviewDirty, setReviewDirty] = useState(false);
  const [markingReviewed, setMarkingReviewed] = useState(false);
  const [focusedItemId, setFocusedItemId] = useState<string | null>(null);
  const [reviewingConflict, setReviewingConflict] = useState(false);
  const [reviewingConflictId, setReviewingConflictId] = useState<string | undefined>(undefined);

  const reviewWindowRef = useRef<WorkstreamReviewWindow | null>(null);
  reviewWindowRef.current = reviewWindow;
  const consumedEntryRef = useRef(false);

  useEffect(() => {
    consumedEntryRef.current = false;
  }, [workstreamId]);

  // Auto-open conflict review modal if entry === "conflicts" and open conflicts exist (one-shot navigation intent)
  useEffect(() => {
    if (!consumedEntryRef.current && entry === "conflicts" && ctx && (ctx.conflicts?.length ?? 0) > 0) {
      consumedEntryRef.current = true;
      setReviewingConflict(true);
    }
  }, [entry, ctx]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.getWorkstreamReviewWindow(workstreamId),
      api.getWorkstreamReviewSummary(workstreamId),
    ]).then(([w, s]) => {
      if (!cancelled) {
        setReviewWindow(w);
        setReviewSummary(s);
        setReviewDirty(s.unseen_change_count !== w.unseen_changes.length);
      }
    }).catch(console.error);
    return () => {
      cancelled = true;
    };
  }, [workstreamId]);

  // Regular contextual or background refresh:
  // Updates summary, but preserves the frozen reviewWindow!
  const refreshSummary = useCallback(() => {
    api.getWorkstreamReviewSummary(workstreamId).then((s) => {
      setReviewSummary(s);
      const cur = reviewWindowRef.current;
      if (cur && s.unseen_change_count > cur.unseen_changes.length) {
        setReviewDirty(true);
      }
    }).catch(console.error);
  }, [workstreamId]);

  useRefreshSignal(refreshSummary);

  // Explicit user refresh of review window
  const handleRefreshReview = useCallback(async () => {
    try {
      const [w, s] = await Promise.all([
        api.getWorkstreamReviewWindow(workstreamId),
        api.getWorkstreamReviewSummary(workstreamId),
      ]);
      setReviewWindow(w);
      setReviewSummary(s);
      setReviewDirty(s.unseen_change_count !== w.unseen_changes.length);
    } catch (err) {
      console.error("Failed to refresh review window:", err);
    }
  }, [workstreamId]);

  // Mark reviewed action:
  // Critical invariant: uses observed reviewWindow.mark_through, NOT a newly fetched frontier!
  const handleMarkReviewed = useCallback(async () => {
    if (!reviewWindow || markingReviewed) return;
    const observed = reviewWindow;
    setMarkingReviewed(true);
    try {
      await api.markWorkstreamReviewed(workstreamId, observed.mark_through);
      const [nextWindow, nextSummary] = await Promise.all([
        api.getWorkstreamReviewWindow(workstreamId),
        api.getWorkstreamReviewSummary(workstreamId),
      ]);
      setReviewWindow(nextWindow);
      setReviewSummary(nextSummary);
      setReviewDirty(nextSummary.unseen_change_count !== nextWindow.unseen_changes.length);
    } catch (err) {
      console.error("Failed to mark workstream reviewed:", err);
    } finally {
      setMarkingReviewed(false);
    }
  }, [workstreamId, reviewWindow, markingReviewed]);

  return (
    <>
      {/* Context 段落并入主栏后纵向堆叠：当前事实在上，冲突 / 变更在下。 */}
      <div className="ws-context-stack">
      <div>
        {ctxState && (
          <div style={{ marginBottom: 18 }}>
            <div className="row between" style={{ alignItems: "center" }}>
              <div className="section-label" style={{ margin: 0 }}>当前 Context</div>
              <div className="row" style={{ gap: 8 }}>
                <button className="btn small ghost" onClick={onCopyContext}>复制</button>
                {ctxState.pending && (
                  <button className="btn small primary" disabled={ctxUpdating} onClick={onUpdateContext}>
                    {ctxUpdating ? "更新中…" : "更新状态"}
                  </button>
                )}
              </div>
            </div>
            {ctxState.pending ? (
              <div className="small muted" style={{ marginTop: 4 }}>
                {ctxState.pending_sessions > 0
                  ? `${ctxState.pending_sessions} 个会话有新内容待并入。`
                  : "有新内容待并入。"}
              </div>
            ) : (
              <div className="small muted" style={{ marginTop: 4 }}>
                已是最新（rev {ctxState.context_revision}）。
              </div>
            )}
            {ctxUpdateError && (
              <div className="badge warn" style={{ marginTop: 8, overflowWrap: "anywhere", display: "flex", gap: 8, alignItems: "center" }}>
                <span>{ctxUpdateError.message}{ctxUpdateError.operationId ? ` · 操作 ID ${ctxUpdateError.operationId}` : ""}</span>
                {ctxUpdateError.operationId && (
                  <button className="btn small ghost" onClick={async () => {
                    const ok = await copyToClipboard(contextUpdateErrorCopyText(ctxUpdateError));
                    showToast(ok ? "已复制错误详情" : "复制失败");
                  }}>复制错误详情</button>
                )}
              </div>
            )}
          </div>
        )}
        {reviewWindow && reviewSummary && (
          <SinceLastReview
            window={reviewWindow}
            summary={reviewSummary}
            dirty={reviewDirty}
            marking={markingReviewed}
            onMarkReviewed={handleMarkReviewed}
            onRefresh={handleRefreshReview}
            onOpenItem={(itemId) => {
              setFocusedItemId(null);
              setTimeout(() => setFocusedItemId(itemId), 20);
            }}
            onOpenConflict={(conflictId) => {
              setReviewingConflictId(conflictId);
              setReviewingConflict(true);
            }}
          />
        )}
        <WorkstreamContext
          ctx={ctx}
          focusedItemId={focusedItemId}
          onChanged={onChanged}
          onNavigateSession={(sessionId) => navigate({ view: "session", sessionId })}
        />
      </div>
      <div>
        <NeedsAttentionSection
          conflicts={ctx.conflicts ?? []}
          onReview={() => {
            setReviewingConflictId(undefined);
            setReviewingConflict(true);
          }}
        />
        <RecentChangesTimeline changes={ctx.recent_changes ?? []} />
      </div>
      </div>

      {reviewingConflict && (
        <ConflictReviewModal
          cases={ctx.conflict_cases ?? []}
          initialConflictId={reviewingConflictId}
          onClose={() => {
            setReviewingConflict(false);
            setReviewingConflictId(undefined);
          }}
          onChanged={onChanged}
        />
      )}
    </>
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
