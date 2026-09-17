import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import {
  AGENT_LABELS,
  type Agent,
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
import { announceLaunch } from "../launcher/LaunchResultModal";

/**
 * Workstream Detail = Understand（整体设计方案 §29-§37）。
 * 第一屏回答：这是什么 / 做到哪 / 目标 / 问题 / 决定 / 约束 / 最近 Session，
 * 并可直接 New、Resume latest、Resume specific、Ask Assistant。
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
  const [reviewingConflict, setReviewingConflict] = useState(false);
  const [reviewingConflictId, setReviewingConflictId] = useState<string | undefined>(undefined);

  // Review loop state: frozen window + summary
  const [reviewWindow, setReviewWindow] = useState<WorkstreamReviewWindow | null>(null);
  const [reviewSummary, setReviewSummary] = useState<WorkstreamReviewSummary | null>(null);
  const [reviewDirty, setReviewDirty] = useState(false);
  const [markingReviewed, setMarkingReviewed] = useState(false);
  const [focusedItemId, setFocusedItemId] = useState<string | null>(null);

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

  // Initial load when workstreamId changes
  useEffect(() => {
    let cancelled = false;
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (!cancelled) setCtx(c);
    }).catch(console.error);

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
  // Updates context & summary, but preserves the frozen reviewWindow!
  // If background changes have increased unseen count beyond current window, marks reviewDirty.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
    api.getWorkstreamReviewSummary(workstreamId).then((s) => {
      setReviewSummary(s);
      const cur = reviewWindowRef.current;
      if (cur && s.unseen_change_count > cur.unseen_changes.length) {
        setReviewDirty(true);
      }
    }).catch(console.error);
  }, [workstreamId]);

  useRefreshSignal(refresh);

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

  useEffect(() => {
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
  }, []);

  if (!ctx) return <div className="main narrow">加载中…</div>;
  const { workstream, related_sessions } = ctx;

  const latest = [...related_sessions]
    .sort((a, b) =>
      (b.last_activity_at ?? b.started_at ?? "").localeCompare(
        a.last_activity_at ?? a.started_at ?? "",
      ),
    )[0];

  const launchNew = async () => {
    if (!defaultAgent || busy) return;
    setBusy(true); setError("");
    try {
      announceLaunch("启动", await api.launchNewSession(defaultAgent, [workstream.id]));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const resumeLatest = async () => {
    if (!latest || busy) return;
    setBusy(true); setError("");
    try {
      announceLaunch("恢复", await api.launchResumeSession(latest.id, []));
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

  const saveCwd = async () => {
    setCwdOpen(false);
    const trimmed = cwdInput.trim();
    await api.updateWorkstream({ ...workstream, default_cwd: trimmed || null });
    refresh();
  };

  return (
    <div className="main narrow">
      <PageHeader
        back="Workstreams"
        onBack={() => navigate({ view: "workstreams" })}
        title={workstream.title}
        actions={
          <>
            <button className="btn ghost"
              onClick={() => navigate({ view: "assistant", scope: { type: "workstream", id: workstream.id } })}>
              Ask Assistant
            </button>
            <div style={{ position: "relative" }}>
              <button className="btn ghost" onClick={() => setMenuOpen((v) => !v)} title="更多操作">•••</button>
              {menuOpen && (
                <div className="menu-pop">
                  <button className="menu-item" onClick={openCwdEditor}>
                    工作目录…
                  </button>
                  <button className="menu-item" onClick={toggleArchive}>
                    {workstream.visibility === "archived" ? "取消归档" : "归档"}
                  </button>
                </div>
              )}
            </div>
            {defaultAgent ? (
              <button className="btn ws-btn" disabled={busy}
                title={`New session with ${AGENT_LABELS[defaultAgent]}`}
                onClick={launchNew}>
                <AgentIcon agent={defaultAgent} />
                {related_sessions.length === 0 ? "Start" : "New"}
              </button>
            ) : (
              <button className="btn ws-btn" disabled
                title="未检测到可用的 Agent CLI — 到 Settings → Agents 配置">
                {related_sessions.length === 0 ? "Start" : "New"}
              </button>
            )}
            {latest && (
              <button className="btn primary ws-btn resume-primary" disabled={busy}
                title={`Resume latest ${AGENT_LABELS[latest.agent]}`}
                onClick={resumeLatest}>
                <AgentIcon agent={latest.agent} />
                {busy ? "启动中…" : "Resume"}
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
          <span className={`badge ${workstream.lifecycle === "open" && workstream.visibility === "normal" ? "success" : ""}`}>
            {lifecycleLabel(workstream)}
          </span>
          {workstream.default_cwd && (
            <>
              <span className="dot-sep" />
              <span className="mono small" title="New Session 默认启动目录（点击 ••• 可修改）">
                {workstream.default_cwd}
              </span>
            </>
          )}
          <span className="dot-sep" />
          <span>Edited {timeAgo(workstream.updated_at)}</span>
          {error && <span style={{ color: "var(--warning)" }}>启动失败</span>}
        </div>
      </PageHeader>

      <div className="ws-detail-grid">
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
            onChanged={refresh}
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
          <WorkstreamSessions sessions={related_sessions} navigate={navigate} />
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
          onChanged={() => {
            refresh();
          }}
        />
      )}

      {cwdOpen && (
        <Modal title="工作目录" onClose={() => setCwdOpen(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            该 Workstream 的 New Session 默认在此目录启动。这只是启动建议，不改变 Workstream 的身份；
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

function lifecycleLabel(w: WorkstreamContextData["workstream"]): string {
  if (w.visibility === "archived") return "Archived";
  if (w.lifecycle === "completed") return "Completed";
  if (w.lifecycle === "abandoned") return "Abandoned";
  return "Active";
}
