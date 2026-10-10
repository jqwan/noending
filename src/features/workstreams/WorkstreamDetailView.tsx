import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import Icon from "../../components/Icon";
import { copyToClipboard, contextUpdateErrorCopyText, contextUpdateErrorDetails, submitsOnEnter, timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import WorkstreamFormModal from "./WorkstreamFormModal";
import {
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamContextView,
  type WorkstreamPathRow,
} from "../../types";
import type { Route } from "../../app/routes";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { MD_COMPONENTS } from "../sessions/SessionMessage";
import WorkstreamSessions from "./WorkstreamSessions";
import WorkstreamPathList from "./WorkspacePaths";

/** Task organization and the current Markdown summary. All reads are pure. */
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
  initialTitle,
  navigate,
  goBack,
}: {
  workstreamId: string;
  initialTitle?: string;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const currentWorkstreamId = useRef(workstreamId);
  currentWorkstreamId.current = workstreamId;
  const cached = workstreamDetailCache.get(workstreamId);
  const [loadError, setLoadError] = useState("");
  const [retry, setRetry] = useState(0);
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(() => cached?.ctx ?? null);
  /** Context 状态（只读）：当前投影 + revision + 待更新。 */
  const [ctxState, setCtxState] = useState<WorkstreamContextView | null>(() => cached?.ctxState ?? null);
  const [failedSessions, setFailedSessions] = useState<{ session_id: string; error: string }[]>([]);
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
    setFailedSessions([]);
    setCtxUpdating(false);
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

  // Background refresh only reads task data and summary state.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (currentWorkstreamId.current !== workstreamId) return;
      setCtx(c);
      const prev = workstreamDetailCache.get(workstreamId);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
        ctxState: prev?.ctxState ?? null,
        paths: prev?.paths ?? null,
      });
    }).catch(console.error);

    api.getWorkstreamContextState(workstreamId).then((s) => {
      if (currentWorkstreamId.current !== workstreamId) return;
      setCtxState(s);
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctxState: s });
      }
    }).catch(console.error);

    api.listWorkstreamPaths(workstreamId).then((rows) => {
      if (currentWorkstreamId.current !== workstreamId) return;
      setPaths(rows);
      setPathsError("");
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, paths: rows });
      }
    }).catch((e) => {
      if (currentWorkstreamId.current !== workstreamId) return;
      setPaths(null);
      setPathsError(`读取工作目录失败：${String(e)}`);
    });
  }, [workstreamId]);

  useRefreshSignal(refresh);

  /** 一次点击 → 相关 Session 摘要与 Workstream 状态一起更新（后端一次模型调用）。 */
  const updateContext = async (regenerate = false) => {
    if (ctxUpdating) return;
    setCtxUpdating(true);
    setCtxUpdateError(null);
    setFailedSessions([]);
    try {
      const out = await (regenerate ? api.regenerateWorkstreamContext(workstreamId) : api.updateWorkstreamContext(workstreamId));
      if (currentWorkstreamId.current !== workstreamId) return;
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
    const ok = await copyToClipboard(ctxState?.summary ?? "");
    showToast(ok ? "已复制摘要" : "复制失败");
  };

  // 同 SessionDetailView：加载态也要渲染 PageHeader，否则整条标题栏会先消失再补回来。
  if (!ctx) {
    if (loadError === "") {
      return (
        <div className="main task-detail" role="status">
          <PageHeader title={initialTitle ?? ""} />
          <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
            <div className="skeleton" style={{ height: 120, borderRadius: "var(--radius-lg)" }} />
            <div className="skeleton" style={{ height: 200, borderRadius: "var(--radius-lg)" }} />
          </div>
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
                title="归档：保留路径、会话与摘要，归档后不能新建会话。"
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

        <section className="rail-section">
          <div className="rail-head">
            <div className="section-label" style={{ margin: 0 }}>任务摘要</div>
            <div className="actions">
              {ctxState?.summary && <button className="btn small ghost" onClick={copyContext}>复制</button>}
              {ctxState?.pending && <button className="btn small primary" disabled={ctxUpdating} onClick={() => updateContext()}>{ctxUpdating ? "更新中…" : "更新摘要"}</button>}
              <button className="btn small ghost" disabled={ctxUpdating} onClick={() => updateContext(true)}>重新生成</button>
            </div>
          </div>
          {ctxState?.pending && <p className="muted">有待更新内容{ctxState.pending_sessions > 0 ? `，涉及 ${ctxState.pending_sessions} 个会话` : ""}</p>}
          {ctxState?.summary ? <div className="md-body" style={{ maxWidth: "72ch", overflowWrap: "anywhere" }}><ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{ctxState.summary}</ReactMarkdown></div> : <p className="muted">尚无任务摘要</p>}
          {failedSessions.length > 0 && <div role="alert"><p>任务摘要已更新，以下会话仍待重试：</p><ul>{failedSessions.map(f => <li key={f.session_id}>{sessions.find(s => s.id === f.session_id)?.title || f.session_id}：{f.error}</li>)}</ul></div>}
          {ctxUpdateError && <div role="alert"><p>{ctxUpdateError.message}</p><button className="btn small ghost" onClick={async () => { await copyToClipboard(contextUpdateErrorCopyText(ctxUpdateError)); }}>复制错误详情</button></div>}
        </section>

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
              onKeyDown={(e) => { if (e.key === "Enter") navigate({ view: "project", projectId: p.id, initialTitle: p.name ?? undefined }); }}
              onClick={() => navigate({ view: "project", projectId: p.id, initialTitle: p.name ?? undefined })}>
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
            它的当前摘要与摘要处理进度。
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
