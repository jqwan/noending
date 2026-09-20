import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import { useWorkstreamCards } from "../workstreams/useWorkstreamCards";
import WorkstreamCard from "../workstreams/WorkstreamCard";
import { sessionDisplayTitle, UNTITLED_SESSION } from "../sessions/SessionTable";
import { timeAgo, useRefreshSignal } from "../../components/common";
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
    .filter((c) => c.lifecycle === "active" && c.visibility === "normal")
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

/** 最近 Sessions：最多 3 条的轻量区域（§21），视觉中心仍是 Workstream。 */
export function RecentSessions({ navigate, onNewSession }: {
  navigate: (r: Route) => void;
  /** 有 Workstream 但还没有任何 Session 时给出的下一步。 */
  onNewSession?: () => void;
}) {
  const [sessions, setSessions] = useState<Session[] | null>(null);

  const refresh = useMemo(
    () => () => {
      api.listSessions().then((ss) => {
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
  // Home 常驻：后台 sync / reconcile 完成后 Recent Sessions 要跟上，
  // 否则会和已刷新的 Workstream 卡片显示不一致的「最新」状态。
  useRefreshSignal(refresh);

  // 还没读到数据时不出声，免得把「加载中」当成「没有 Session」。
  if (!sessions) return null;

  return (
    <div className="recent-sessions" style={{ marginTop: 36 }}>
      <div className="section-label">最近 Sessions</div>

      {/* Workstream 存在但一次都还没跑过：这里必须留下一个明确的下一步，
          否则首页看起来像空的。 */}
      {sessions.length === 0 ? (
        <div className="muted small">
          还没有 Session。{" "}
          {onNewSession && (
            <button className="link small" onClick={onNewSession}>新建 Session</button>
          )}
        </div>
      ) : (
        sessions.map((s) => (
          <div className="list-row" key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
            <div className="grow">
              <div className="title" title={s.title ?? `${UNTITLED_SESSION} · ${s.agent_session_id}`}>
                {sessionDisplayTitle(s.title)}
              </div>
            </div>
            <div className="side">
              <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
              <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
            </div>
          </div>
        ))
      )}
    </div>
  );
}
