import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal } from "../../components/common";
import { AGENT_LABELS, type Agent, type LaunchResult, type WorkstreamContext as WorkstreamContextData } from "../../types";
import type { Route } from "../../app/routes";
import WorkstreamContext from "./WorkstreamContext";
import WorkstreamSessions from "./WorkstreamSessions";
import WorkstreamActivity from "./WorkstreamActivity";
import LaunchResultModal from "../launcher/LaunchResultModal";

/**
 * Workstream Detail = Understand（整体设计方案 §29-§37）。
 * 第一屏回答：这是什么 / 做到哪 / 目标 / 问题 / 决定 / 约束 / 最近 Session，
 * 并可直接 New、Resume latest、Resume specific、Ask Assistant。
 */
export default function WorkstreamDetailView({ workstreamId, navigate }: {
  workstreamId: string;
  navigate: (r: Route) => void;
}) {
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(null);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [error, setError] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);

  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
  }, [workstreamId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);
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
      setResult(await api.launchNewSession(defaultAgent, [workstream.id]));
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
      setResult(await api.launchResumeSession(latest.id, []));
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
                  <button className="menu-item" onClick={toggleArchive}>
                    {workstream.visibility === "archived" ? "取消归档" : "归档"}
                  </button>
                </div>
              )}
            </div>
            {defaultAgent && (
              <button className="btn ws-btn" disabled={busy}
                title={`New session with ${AGENT_LABELS[defaultAgent]}`}
                onClick={launchNew}>
                <AgentIcon agent={defaultAgent} />
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
          <span className="dot-sep" />
          <span>Edited {timeAgo(workstream.updated_at)}</span>
          {error && <span style={{ color: "var(--warning)" }}>启动失败</span>}
        </div>
      </PageHeader>

      <div className="ws-detail-grid">
        <div>
          <WorkstreamContext ctx={ctx} onChanged={refresh} />
        </div>
        <div>
          <WorkstreamSessions sessions={related_sessions} navigate={navigate} />
          <WorkstreamActivity items={ctx.items} />
        </div>
      </div>

      {result && <LaunchResultModal result={result} onClose={() => setResult(null)} />}
    </div>
  );
}

function lifecycleLabel(w: WorkstreamContextData["workstream"]): string {
  if (w.visibility === "archived") return "Archived";
  if (w.lifecycle === "completed") return "Completed";
  if (w.lifecycle === "abandoned") return "Abandoned";
  return "Active";
}
