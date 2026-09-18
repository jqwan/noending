import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { IntelligenceOnly } from "../../app/experience";
import NewSessionModal from "../sessions/NewSessionModal";
import ResumeSessionModal from "../sessions/ResumeSessionModal";
import {
  AGENT_LABELS,
  type Agent,
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamReviewSummary,
  type WorkstreamReviewWindow,
} from "../../types";
import type { Route, WorkstreamEntry } from "../../app/routes";
import WorkstreamContext from "./WorkstreamContext";
import WorkstreamSessions from "./WorkstreamSessions";
import NeedsAttentionSection from "./NeedsAttentionSection";
import ConflictReviewModal from "./ConflictReviewModal";
import RecentChangesTimeline from "./RecentChangesTimeline";
import SinceLastReview from "./SinceLastReview";

/**
 * Workstream Detail = 持续相关 Sessions 的组织容器（方案 §14）。
 *
 * Base Experience 下这一页只有五块内容：概览（描述）、Sessions、Project、
 * 工作目录、状态。Context 智能段落（Current Context / Since Last Review /
 * Needs Attention / Recent Changes / Conflict Review）全部保留代码但
 * 不挂载——见 §11.9，off 就是 `<IntelligenceOnly>` 里不渲染。
 *
 * 启动路径唯一：本页不再自己调 launcher，而是挂载 New Session / Resume 的
 * 同一个 Modal（§8.1.1 契约），由它们走 prepare → 状态指纹 → launch_prepared
 * （Preview-Launch Identity / Launch Preparation Integrity）。
 */
export default function WorkstreamDetailView({
  workstreamId,
  entry,
  navigate,
}: {
  workstreamId: string;
  entry?: WorkstreamEntry;
  navigate: (r: Route) => void;
}) {
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(null);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const [cwdOpen, setCwdOpen] = useState(false);
  const [cwdInput, setCwdInput] = useState("");
  const [titleOpen, setTitleOpen] = useState(false);
  const [titleInput, setTitleInput] = useState("");
  const [editingDescription, setEditingDescription] = useState(false);
  const [descriptionInput, setDescriptionInput] = useState("");
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [resumeSessionId, setResumeSessionId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (!cancelled) setCtx(c);
    }).catch(console.error);
    return () => {
      cancelled = true;
    };
  }, [workstreamId]);

  useEffect(() => {
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
  }, []);

  // Background refresh only re-reads the projection; the intelligence panels
  // own their own (frozen-window) refresh so this page never touches ReviewState.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
  }, [workstreamId]);

  useRefreshSignal(refresh);

  if (!ctx) return <div className="main narrow">加载中…</div>;
  const { workstream, related_sessions } = ctx;

  const latest = [...related_sessions]
    .sort((a, b) =>
      (b.last_activity_at ?? b.started_at ?? "").localeCompare(
        a.last_activity_at ?? a.started_at ?? "",
      ),
    )[0];

  /**
   * `update_workstream` is a whole-object write, so every edit re-sends the
   * current record with one field replaced. title / description / updated_at
   * are part of the launch state fingerprint (launcher/mod.rs:475-478): an
   * edit here intentionally invalidates any not-yet-consumed PreparedLaunch,
   * which surfaces as 「状态已变化」 in the launch modal rather than being
   * silently absorbed.
   */
  const save = async (patch: Partial<Workstream>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await api.updateWorkstream({ ...workstream, ...patch });
      refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggleArchive = async () => {
    setMenuOpen(false);
    await api.archiveWorkstream(workstream.id);
    navigate({ view: "workstreams" });
  };

  const openCwdEditor = () => {
    setMenuOpen(false);
    setCwdInput(workstream.default_cwd ?? "");
    setCwdOpen(true);
  };

  const openTitleEditor = () => {
    setMenuOpen(false);
    setTitleInput(workstream.title);
    setTitleOpen(true);
  };

  const openDescriptionEditor = () => {
    setMenuOpen(false);
    setDescriptionInput(workstream.description ?? "");
    setEditingDescription(true);
  };

  const saveCwd = async () => {
    setCwdOpen(false);
    const trimmed = cwdInput.trim();
    await save({ default_cwd: trimmed || null });
  };

  const saveTitle = async () => {
    const trimmed = titleInput.trim();
    if (!trimmed || trimmed === workstream.title) {
      setTitleOpen(false);
      return;
    }
    setTitleOpen(false);
    await save({ title: trimmed });
  };

  const saveDescription = async () => {
    setEditingDescription(false);
    await save({ description: descriptionInput.trim() });
  };

  const archived = workstream.visibility === "archived";

  return (
    <div className="main narrow">
      <PageHeader
        back="Workstreams"
        onBack={() => navigate({ view: "workstreams" })}
        title={<span style={{ overflowWrap: "anywhere" }}>{workstream.title}</span>}
        actions={
          <>
            <div style={{ position: "relative" }}>
              <button className="btn ghost" onClick={() => setMenuOpen((v) => !v)} title="更多操作">
                •••
              </button>
              {menuOpen && (
                <div className="menu-pop">
                  <button className="menu-item" onClick={openTitleEditor}>
                    重命名…
                  </button>
                  <button className="menu-item" onClick={openDescriptionEditor}>
                    编辑描述…
                  </button>
                  <button className="menu-item" onClick={openCwdEditor}>
                    工作目录…
                  </button>
                  <button className="menu-item" onClick={toggleArchive}>
                    {archived ? "取消归档" : "归档"}
                  </button>
                </div>
              )}
            </div>
            {defaultAgent ? (
              <button
                className="btn ws-btn"
                title={`用 ${AGENT_LABELS[defaultAgent]} 新建 Session`}
                onClick={() => setNewSessionOpen(true)}
              >
                <AgentIcon agent={defaultAgent} />
                新建 Session
              </button>
            ) : (
              <button
                className="btn ws-btn"
                disabled
                title="未检测到可用的 Agent CLI — 到 设置 → Agent 配置"
              >
                新建 Session
              </button>
            )}
            {latest && (
              <button
                className="btn primary ws-btn resume-primary"
                title={`继续最近的 ${AGENT_LABELS[latest.agent]} Session`}
                onClick={() => setResumeSessionId(latest.id)}
              >
                <AgentIcon agent={latest.agent} />
                继续
              </button>
            )}
          </>
        }
      >
        <div className="ws-detail-head-meta">
          {ctx.project_name && (
            <>
              <button className="link" onClick={() => workstream.project_id && navigate({ view: "project", projectId: workstream.project_id })}>
                {ctx.project_name}
              </button>
              <span className="dot-sep" />
            </>
          )}
          <span className={`badge ${!archived && workstream.lifecycle === "open" ? "success" : ""}`}>
            {lifecycleLabel(workstream)}
          </span>
          {workstream.default_cwd && (
            <>
              <span className="dot-sep" />
              <span className="mono small" title="新建 Session 的默认工作目录（点击 ••• 可修改）">
                {workstream.default_cwd}
              </span>
            </>
          )}
          <span className="dot-sep" />
          <span>最近更新 {timeAgo(workstream.updated_at)}</span>
          {error && <span style={{ color: "var(--warning)" }}>保存失败</span>}
        </div>
      </PageHeader>

      {/* ---------- Base Experience：Workstream 自身（§14） ---------- */}
      <div style={{ marginTop: 26 }}>
        <section className="rail-section">
          <div className="rail-head">
            <div className="section-label" style={{ margin: 0 }}>Workstream 概览</div>
            {!editingDescription && (
              <button className="link" onClick={openDescriptionEditor}>编辑描述</button>
            )}
          </div>
          {editingDescription ? (
            <div className="ctx-edit">
              <textarea
                value={descriptionInput}
                autoFocus
                placeholder="这件 Workstream 想持续做什么（可选）"
                onChange={(e) => setDescriptionInput(e.target.value)}
              />
              <div className="row" style={{ justifyContent: "flex-end" }}>
                <button className="btn" onClick={() => setEditingDescription(false)}>取消</button>
                <button className="btn primary" disabled={busy} onClick={saveDescription}>保存</button>
              </div>
            </div>
          ) : workstream.description ? (
            <p style={{ margin: 0, maxWidth: "72ch", overflowWrap: "anywhere" }}>
              {workstream.description}
            </p>
          ) : (
            <div className="l1-none">还没有描述。</div>
          )}
        </section>

        <WorkstreamSessions
          sessions={related_sessions}
          navigate={navigate}
          onNewSession={() => setNewSessionOpen(true)}
        />

        <section className="rail-section">
          <div className="section-label">Project</div>
          {workstream.project_id ? (
            <button
              className="link"
              style={{ padding: 0 }}
              onClick={() => navigate({ view: "project", projectId: workstream.project_id! })}
            >
              {ctx.project_name ?? "未知 Project"}
            </button>
          ) : (
            <div className="l1-none">不归属任何 Project — Workstream 可以独立存在。</div>
          )}
        </section>

        <section className="rail-section">
          <div className="section-label">工作目录</div>
          {workstream.default_cwd ? (
            <div className="mono" style={{ overflowWrap: "anywhere" }}>{workstream.default_cwd}</div>
          ) : (
            <div className="l1-none">未设置 — 新建 Session 时按最近活动的 Session 目录推断。</div>
          )}
          <div className="small muted" style={{ marginTop: 4 }}>
            这只是新建 Session 的启动建议，不决定 Workstream 的身份。
          </div>
        </section>

        <section className="rail-section">
          <div className="section-label">状态</div>
          <div className="row" style={{ gap: 8 }}>
            <span className={`badge ${workstream.lifecycle === "open" ? "success" : ""}`}>
              {LIFECYCLE_LABELS[workstream.lifecycle] ?? workstream.lifecycle}
            </span>
            {archived && <span className="badge">已归档</span>}
          </div>
          <div className="small muted" style={{ marginTop: 4 }}>
            创建于 {formatDate(workstream.created_at)} · 最近更新 {timeAgo(workstream.updated_at)}
          </div>
        </section>
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

      {resumeSessionId && (
        <ResumeSessionModal
          sessionId={resumeSessionId}
          onClose={() => {
            setResumeSessionId(null);
            refresh();
          }}
        />
      )}

      {titleOpen && (
        <Modal title="重命名 Workstream" onClose={() => setTitleOpen(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            标题是用户可见的组织信息。修改会让已经预览过、但还没启动的那次
            Session 变成「状态已变化」，需要你重新确认——这是刻意保留的保护。
          </p>
          <label className="field"><span>标题</span>
            <input type="text" value={titleInput} autoFocus
              onChange={(e) => setTitleInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && saveTitle()} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setTitleOpen(false)}>取消</button>
            <button className="btn primary" disabled={!titleInput.trim()} onClick={saveTitle}>保存</button>
          </div>
        </Modal>
      )}

      {cwdOpen && (
        <Modal title="工作目录" onClose={() => setCwdOpen(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            该 Workstream 的新建 Session 默认在此目录启动。这只是启动建议，不改变 Workstream 的身份；
            留空则按「最近活动的 Session 目录」推断。
          </p>
          <label className="field"><span>路径</span>
            <input type="text" className="mono" value={cwdInput} autoFocus
              onChange={(e) => setCwdInput(e.target.value)}
              placeholder="/path/to/project"
              onKeyDown={(e) => e.key === "Enter" && saveCwd()} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setCwdOpen(false)}>取消</button>
            <button className="btn primary" onClick={saveCwd}>保存</button>
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

// §2.1 / §2.2 词表：Workstream 的 open → 进行中，archived → 已归档。
const LIFECYCLE_LABELS: Record<Workstream["lifecycle"], string> = {
  open: "进行中",
  completed: "已完成",
  abandoned: "已放弃",
};

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

function lifecycleLabel(w: Workstream): string {
  if (w.visibility === "archived") return "已归档";
  return LIFECYCLE_LABELS[w.lifecycle] ?? w.lifecycle;
}
