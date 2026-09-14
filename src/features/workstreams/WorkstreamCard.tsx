import React, { useState } from "react";
import { api } from "../../api";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import type { Route } from "../../app/routes";
import { AGENT_LABELS, type Agent, type LaunchResult, type WorkstreamCardData } from "../../types";
import LaunchResultModal from "../launcher/LaunchResultModal";

const LIFECYCLE_LABELS: Record<string, string> = {
  completed: "已完成",
  abandoned: "已搁置",
};

/**
 * The one Workstream card shared by Home (compact) and Workstreams (full).
 * Behavior is identical in both modes:
 *   Body   → Workstream Detail
 *   New    → launch a New Session with the default agent (Start when the
 *            Workstream has no sessions yet)
 *   Resume → resume the latest session of its own agent (hidden when none)
 */
export default function WorkstreamCard({ card, mode, navigate, defaultAgent }: {
  card: WorkstreamCardData;
  mode: "compact" | "full";
  navigate: (r: Route) => void;
  defaultAgent: Agent | null;
}) {
  const [busy, setBusy] = useState<"new" | "resume" | null>(null);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [error, setError] = useState("");

  const openDetail = () => navigate({ view: "workstream", workstreamId: card.id });

  const newSession = async () => {
    if (!defaultAgent || busy) return;
    setBusy("new");
    setError("");
    try {
      setResult(await api.launchNewSession(defaultAgent, [card.id]));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const resumeSession = async () => {
    if (!card.latest_session || busy) return;
    setBusy("resume");
    setError("");
    try {
      setResult(await api.launchResumeSession(card.latest_session.id, []));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  // Current State → Description → Goal（§4 内容优先级，两行截断）
  const body = card.current_state || card.description || card.goal || "";
  const newLabel = card.session_count === 0 ? "Start" : "New";

  return (
    <article className={`ws-card ${mode}`} onClick={openDetail}>
      <header className="ws-card-head">
        <h3 className="ws-card-title">{card.title}</h3>
        <div className="ws-card-side">
          {card.lifecycle !== "open" && (
            <span className="badge">{LIFECYCLE_LABELS[card.lifecycle] ?? card.lifecycle}</span>
          )}
          {/* Project 是可选组织层：未归属时不显示任何占位 */}
          {card.project_name && <span className="ws-card-project">{card.project_name}</span>}
        </div>
      </header>

      {body ? (
        <p className="ws-card-body">{body}</p>
      ) : (
        <p className="ws-card-body muted">还没有 Current State — 在详情页补充目标与进展。</p>
      )}

      <footer className="ws-card-meta">
        <span>
          {card.session_count === 0
            ? "No sessions yet"
            : `Last active ${timeAgo(card.last_activity_at)} · ${card.session_count} sessions`}
        </span>
        <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
          {error && <span className="ws-card-error" title={error}>启动失败</span>}
          {defaultAgent && (
            <button
              className="btn small ws-btn"
              disabled={busy !== null}
              title={`New session with ${AGENT_LABELS[defaultAgent]}`}
              onClick={newSession}
            >
              <AgentIcon agent={defaultAgent} />
              {busy === "new" ? "启动中…" : newLabel}
            </button>
          )}
          {card.latest_session && (
            <button
              className={`btn small ws-btn ${mode === "compact" ? "resume-primary" : ""}`}
              disabled={busy !== null}
              title={`Resume latest ${AGENT_LABELS[card.latest_session.agent]} session`}
              onClick={resumeSession}
            >
              <AgentIcon agent={card.latest_session.agent} />
              {busy === "resume" ? "启动中…" : "Resume"}
            </button>
          )}
        </div>
      </footer>

      {result && <LaunchResultModal result={result} onClose={() => setResult(null)} />}
    </article>
  );
}
