import React, { useState } from "react";
import { api } from "../../api";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import LaunchResultModal from "../launcher/LaunchResultModal";
import { AGENT_LABELS, type LaunchResult, type Session } from "../../types";
import type { Route } from "../../app/routes";

/**
 * 右栏 Sessions（整体设计方案 §35）：每行可以 Resume 指定 Session，
 * 与 Header 的「Resume latest」区分（实施方案 §28）。
 */
export default function WorkstreamSessions({ sessions, navigate }: {
  sessions: Session[];
  navigate: (r: Route) => void;
}) {
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  const sorted = [...sessions].sort((a, b) =>
    (b.last_activity_at ?? b.started_at ?? "").localeCompare(
      a.last_activity_at ?? a.started_at ?? "",
    ),
  );

  const resume = async (sessionId: string) => {
    setBusy(sessionId);
    setError("");
    try {
      setResult(await api.launchResumeSession(sessionId, []));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>Sessions</div>
        <button className="link" onClick={() => navigate({ view: "sessions" })}>View all →</button>
      </div>
      {sorted.length === 0 && (
        <div className="l1-none">还没有 Session 关联到这里。</div>
      )}
      {sorted.slice(0, 8).map((s) => (
        <div className="rail-row" key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
          <div className="rail-main">
            <div className="rail-title">{s.title ?? s.agent_session_id}</div>
            <div className="rail-sub">{timeAgo(s.last_activity_at ?? s.started_at)}</div>
          </div>
          <button className="btn small" disabled={busy !== null}
            onClick={(e) => { e.stopPropagation(); resume(s.id); }}>
            {busy === s.id ? "…" : "Resume"}
          </button>
        </div>
      ))}
      {error && <div className="muted small" style={{ color: "var(--warning)" }}>{error}</div>}
      {result && <LaunchResultModal result={result} onClose={() => setResult(null)} />}
    </div>
  );
}
