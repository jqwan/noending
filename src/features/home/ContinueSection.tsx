import React, { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import { useWorkstreamCards } from "../workstreams/useWorkstreamCards";
import WorkstreamCard from "../workstreams/WorkstreamCard";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Session } from "../../types";
import type { Route } from "../../app/routes";

/** Continue 区：最近 4 个（最多 6）Workstream，双列网格（§12/§13）。 */
export function ContinueSection({ navigate, defaultAgent, cards }: {
  navigate: (r: Route) => void;
  defaultAgent: ReturnType<typeof useWorkstreamCards>["defaultAgent"];
  cards: NonNullable<ReturnType<typeof useWorkstreamCards>["cards"]>;
}) {
  const active = cards
    .filter((c) => c.lifecycle === "open" && c.visibility === "normal")
    .slice(0, 6);

  if (active.length === 0) return null;

  return (
    <>
      <div className="page-head" style={{ marginBottom: 12 }}>
        <div className="section-label" style={{ margin: 0 }}>最近活动</div>
        <button className="btn small ghost" onClick={() => navigate({ view: "workstreams" })}>
          Workstreams →
        </button>
      </div>
      <div className="ws-grid">
        {active.map((c) => (
          <WorkstreamCard key={c.id} card={c} mode="compact" navigate={navigate} defaultAgent={defaultAgent} />
        ))}
      </div>
    </>
  );
}

/** 最近会话：最多 3 条的轻量区域（§21），视觉中心仍是 Workstream。 */
export function RecentSessions({ navigate }: { navigate: (r: Route) => void }) {
  const [sessions, setSessions] = useState<Session[] | null>(null);

  const refresh = useMemo(
    () => () => {
      api.listAllSessions().then((ss) => {
        const sorted = [...ss]
          .sort((a, b) =>
            (b.last_activity_at ?? b.started_at ?? "").localeCompare(
              a.last_activity_at ?? a.started_at ?? "",
            ),
          )
          .slice(0, 3);
        setSessions(sorted);
      }).catch(console.error);
    },
    [],
  );
  useEffect(refresh, [refresh]);

  if (!sessions || sessions.length === 0) return null;

  return (
    <div className="recent-sessions" style={{ marginTop: 36 }}>
      <div className="section-label">最近会话</div>
      {sessions.map((s) => (
        <div className="list-row" key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <div className="grow">
            <div className="title">{s.title ?? s.agent_session_id}</div>
          </div>
          <div className="side">
            <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
            <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
