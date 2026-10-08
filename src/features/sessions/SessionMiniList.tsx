import { useState } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { sessionDisplayTitle, UNTITLED_SESSION } from "./SessionTable";
import { AGENT_LABELS, type Session } from "../../types";
import type { Route } from "../../app/routes";

/**
 * 项目详情与任务详情共用的会话迷你列表：整行点击进会话详情，行内无动作
 * （继续 / 已归档在会话页头部）。展示与交互两处逐字节一致——Agent 图标 +
 * 标题一行，右侧相对时间；默认 12 条，「查看全部 / 收起」展开。
 */
export default function SessionMiniList({ sessions, emptyText, navigate }: {
  sessions: Session[];
  emptyText: string;
  navigate: (r: Route) => void;
}) {
  const [showAll, setShowAll] = useState(false);

  return (
    <>
      {sessions.length === 0 && <div className="l1-none">{emptyText}</div>}
      {sessions.slice(0, showAll ? undefined : 12).map((s) => (
        <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <div className="grow">
            <div className="title mini-title" title={s.title ?? `${UNTITLED_SESSION} · ${s.root_agent_session_id}`}>
              <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
              <span className="truncate">{sessionDisplayTitle(s.title)}</span>
            </div>
          </div>
          <div className="side">
            <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
          </div>
        </div>
      ))}
      {sessions.length > 12 && (
        <div className="small muted" style={{ marginTop: 6 }}>
          <button className="btn small ghost" onClick={() => setShowAll(value => !value)}>{showAll ? "收起" : `查看全部 ${sessions.length} 个会话`}</button>
        </div>
      )}
    </>
  );
}
