import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { IntelligenceOnly } from "../../app/experience";
import NewSessionModal from "../sessions/NewSessionModal";
import WorkstreamFormModal from "./WorkstreamFormModal";
import {
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamLifecycle,
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
 * Workstream Detail = 持续相关 Sessions 的组织容器（方案 §14）。
 *
 * Base Experience 下这一页只有五块内容：概览（描述）、Sessions、工作目录、
 * Project、状态。Context 智能段落（Current Context / Since Last Review /
 * Needs Attention / Recent Changes / Conflict Review）全部保留代码但不挂载
 * ——见 §11.9，off 就是 `<IntelligenceOnly>` 里不渲染。
 *
 * 启动路径唯一：本页不再自己调 launcher，而是挂载 New Session / Resume 的
 * 同一个 Modal（§8.1.1 契约），由它们走 prepare → 状态指纹 → launch_prepared
 * （Preview-Launch Identity / Launch Preparation Integrity）。
 *
 * 编辑入口唯一：标题 / 描述 / 工作目录都在 ••• → 编辑任务（WorkstreamFormModal）。
 *
 * v0.2 的边界（§1.5 / §1.13 / §42.3-M19）：
 *   • 工作目录 = **有序 WorkstreamPath 列表**；
 *   • Project 是**只读投影**（主路径 → WorkspacePath → Project），所以链接取的是
 *     列表第 1 条路径所属 Project；
 *   • 归档 / 恢复 / 永久删除是三个**单向**命令，不是一枚翻转开关。
 */
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
  const [loadError, setLoadError] = useState("");
  const [retry, setRetry] = useState(0);
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(null);
  const [paths, setPaths] = useState<WorkstreamPathRow[] | null>(null);
  const [pathsError, setPathsError] = useState("");
  const [busy, setBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [actionError, setActionError] = useState("");
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const [purgeConfirmText, setPurgeConfirmText] = useState("");
  const busyRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // ••• 菜单：点击外部与 Escape 都要收起。之前只有再点一次 ••• 才关得掉，
  // 点别处它一直悬着（§25 交互一致性）。mousedown 阶段监听，先于 click，
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
    // 换 Workstream 时先把手里的投影清空：本页所有写操作都以 workstreamId 为准，
    // 留着上一条的路径列表会让用户在错的列表上按下按钮（例如把旧 path id 发给
    // 新 Workstream 的 reorder）。宁可闪一下「加载中…」，也不拿旧数据当现状。
    setCtx(null);
    setLoadError("");
    setPaths(null);
    setPathsError("");
    setMenuOpen(false);
    setEditing(false);
    setConfirmTrash(false);
    setConfirmPurge(false);
    setActionError("");
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (!cancelled) setCtx(c);
    }).catch(e => { if (!cancelled) setLoadError(String(e)); });
    api.listWorkstreamPaths(workstreamId).then((rows) => {
      if (!cancelled) { setPaths(rows); setPathsError(""); }
    }).catch((e) => {
      if (!cancelled) { setPaths(null); setPathsError(`读取工作目录失败：${String(e)}`); }
    });
    return () => {
      cancelled = true;
    };
  }, [workstreamId, retry]);

  // Background refresh only re-reads the projection; the intelligence panels
  // own their own (frozen-window) refresh so this page never touches ReviewState.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
    api.listWorkstreamPaths(workstreamId).then((rows) => {
      setPaths(rows); setPathsError("");
    }).catch((e) => {
      setPaths(null); setPathsError(`读取工作目录失败：${String(e)}`);
    });
  }, [workstreamId]);

  useRefreshSignal(refresh);

  // 同 SessionDetailView：加载态也要渲染 PageHeader，否则整条标题栏会先消失再补回来。
  // 标题承担"是什么状态"，正文只放具体的错误详情，不再重复一遍状态名。
  if (!ctx) {
    return (
      <div className="main narrow" role="status">
        <PageHeader title={loadError !== "" ? "读取任务失败" : "加载中…"}>
          {loadError !== "" && (
            <>
              <p>{loadError}</p>
              <button className="btn" onClick={() => setRetry(value => value + 1)}>重试</button>
            </>
          )}
        </PageHeader>
      </div>
    );
  }
  const { workstream, sessions } = ctx;

  /**
   * 一条命令返回新的 Workstream 时立刻就地替换：`update_workstream` 是整对象写，
   * 手里留着旧的 lifecycle / visibility，下一次整对象保存就会被后端拒绝。
   */
  const adopt = (next: Workstream) => setCtx((c) => (c ? { ...c, workstream: next } : c));

  /**
   * §1.13：lifecycle 只是分类，没有任何行为差异，随时可切；它不碰路径、
   * 会话归属、visibility 或 Context。
   */
  const setLifecycle = async (next: WorkstreamLifecycle) => {
    if (busyRef.current || workstream.lifecycle === next) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.setWorkstreamLifecycle(workstream.id, next));
      refresh();
    } catch (e) {
      console.error(e);
      setActionError(`修改状态失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /**
   * 归档与恢复是**两个单向命令**（§43.4-1）：`archive_workstream` 只进回收站，
   * `restore_workstream` 只出来。旧代码把 archive 当翻转用，于是「再点一次取消
   * 归档」和「重复点击」会互相抵消 —— 那正是 v0.2 要消掉的双权威。
   * 之前这里 `await` 完不判成败就跳走：失败时菜单收起、页面不动、没有任何
   * 回执。现在失败留在原地说明原因，成功才跳走。
   */
  const moveToTrash = async () => {
    setMenuOpen(false);
    setConfirmTrash(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.archiveWorkstream(workstream.id));
      goBack({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`移入回收站失败：${String(e)}`);
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

  /** 唯一不可逆的动作，且后端只接受从回收站出发（§1.13）。 */
  const purge = async () => {
    setConfirmPurge(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      await api.deleteWorkstreamPermanently(workstream.id);
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

  /**
   * 工作目录还没读回来时不能进编辑模式：草稿由当前列表预填，拿「空」当「没有目录」
   * 会在保存时把已有路径全删掉。所以这里明说还在读，而不是静默什么都不做。
   */
  const openEditor = () => {
    setMenuOpen(false);
    if (paths === null) {
      setActionError("工作目录还在读取中，请稍后再试。");
      return;
    }
    setEditing(true);
  };

  const archived = workstream.visibility === "archived";
  // §1.12：一个 Workstream 可以因为不同路径同时出现在多个 Project 里；
  // 经由 position 0 那条路径到达的才是「主关联」。
  const projectRows = (() => {
    const byId = new Map<string, { id: string; name: string | null; primary: boolean; count: number }>();
    for (const p of paths ?? []) {
      const seen = byId.get(p.project_id);
      byId.set(p.project_id, {
        id: p.project_id,
        name: p.project_name ?? seen?.name ?? null,
        primary: (seen?.primary ?? false) || p.position === 0,
        count: (seen?.count ?? 0) + 1,
      });
    }
    return [...byId.values()].sort((a, b) => (b.primary ? 1 : 0) - (a.primary ? 1 : 0));
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
              <button className="btn ghost icon-button" aria-label="移入回收站"
                title="移入回收站：只是不再出现在列表里，路径、会话归属与 Context 都原样保留，随时可以恢复。"
                onClick={() => setConfirmTrash(true)}>
                <Icon name="trash" />
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
                      从回收站恢复
                    </button>
                    <button className="menu-item"
                      title="不可撤销：会删除这条任务名下的 Context、冲突记录与审阅状态。会话与它们的事件历史保留。"
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

      {/* ---------- Base Experience：Workstream 自身（§14） ---------- */}
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
          onNewSession={archived ? undefined : () => setNewSessionOpen(true)}
          allowActions={!archived}
        />

        </div>
        <aside className="task-detail-aside">
        <section className="rail-section">
          <div className="section-label">状态</div>
          <div className="row" style={{ gap: 8 }}>
            <button className={`btn small ${workstream.lifecycle === "active" ? "primary" : ""}`}
              disabled={busy || workstream.lifecycle === "active"}
              onClick={() => setLifecycle("active")}>
              进行中
            </button>
            <button className={`btn small ${workstream.lifecycle === "completed" ? "primary" : ""}`}
              disabled={busy || workstream.lifecycle === "completed"}
              onClick={() => setLifecycle("completed")}>
              已完成
            </button>
            {archived && <span className="badge warn">在回收站中</span>}
          </div>
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
                <div className="meta">
                  {p.primary ? "主项目" : "关联项目"}
                  {p.count > 1 ? ` · ${p.count} 条路径` : ""}
                </div>
              </div>
            </div>
          ))}

        </section>
        </aside>
      </div>

      {/* ---------- 智能段落：off 时整块不挂载（§11.9、§14） ---------- */}
      <IntelligenceOnly>
        <div className="ws-detail-grid">
          <IntelligenceSections
            ctx={ctx}
            entry={entry}
            workstreamId={workstreamId}
            navigate={navigate}
            onChanged={refresh}
          />
        </div>
      </IntelligenceOnly>

      {newSessionOpen && (
        <NewSessionModal
          workstreamId={workstream.id}
          onClose={() => {
            setNewSessionOpen(false);
            refresh();
          }}
        />
      )}

      {editing && paths !== null && (
        <WorkstreamFormModal
          workstream={workstream}
          paths={paths}
          onClose={() => setEditing(false)}
          onSaved={refresh}
        />
      )}

      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => setConfirmTrash(false)}>
          <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
            <b>{workstream.title}</b> 会离开正常列表，出现在任务页的「回收站」筛选里。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            路径、会话归属和上下文都会保留，可从回收站恢复。
          </p>

          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmTrash(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={moveToTrash} disabled={busy}>
              {busy ? "处理中…" : "移入回收站"}
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
            只有已经在回收站里的任务才能被永久删除 —— 这是刻意留的缓冲。
          </p>
          <label className="field"><span>输入这项任务的标题以确认</span>
            <input type="text" value={purgeConfirmText} autoFocus
              placeholder={workstream.title}
              onChange={(e) => setPurgeConfirmText(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229 && purgeConfirmText === workstream.title && purge()} /></label>
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

/**
 * Context 智能段落。单独成一个组件，是为了让这些开关只在「真的挂载」时才发生：
 * getWorkstreamReviewWindow / Summary / markWorkstreamReviewed / conflict 命令在
 * Base Experience 下不出请求（§34），而不是发了请求再藏起来。
 */
function IntelligenceSections({
  ctx,
  entry,
  workstreamId,
  navigate,
  onChanged,
}: {
  ctx: WorkstreamContextData;
  entry?: WorkstreamEntry;
  workstreamId: string;
  navigate: (r: Route) => void;
  onChanged: () => void;
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
      <div>
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
 * RFC3339 (UTC) → 本地 YYYY/MM/DD.
 *
 * 不用 `toLocaleDateString()`：webview 语言不一定是中文，格式会随环境漂移；
 * 也不用 `slice(0, 10)`：那是 UTC 日期，本地可能已经跨了一天。
 */
function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getFullYear()}/${pad(t.getMonth() + 1)}/${pad(t.getDate())}`;
}
