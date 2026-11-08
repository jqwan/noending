import { recentFirst } from "../../components/detailOrder";
import { DetailList, DetailListRow } from "../../components/DetailSection";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { sessionDisplayTitle, UNTITLED_SESSION, SessionResumeButton } from "./SessionTable";
import { continueSessionDesktopWithToast, resolveSessionContinueMode, useAgentStatus } from "./continueDesktop";
import { AGENT_LABELS, type SessionListItem, type TerminalSummary } from "../../types";
import { useTerminalResumeStates } from "./useTerminalResumeStates";
import { useSessionTerminals } from "./useSessionTerminals";
import type { Route } from "../../app/routes";

/**
 * 详情页共用的会话列表。
 * 主入口打开会话详情，时间是补充信息，继续会话是独立操作；
 * 按最近活动倒序，在固定上限的内容区浏览完整列表。
 */
export default function SessionMiniList({ sessions, emptyText, navigate, terminals: externalTerminals }: {
  sessions: SessionListItem[];
  emptyText: string;
  navigate: (r: Route) => void;
  terminals?: TerminalSummary[];
}) {
  const agentStatus = useAgentStatus();
  const { terminals: polledTerminals } = useSessionTerminals();
  const terminals = externalTerminals ?? polledTerminals;
  const visibleSessions = recentFirst(sessions, session => session.last_activity_at ?? session.started_at);
  const resumeStates = useTerminalResumeStates(visibleSessions
    .filter(s => !s.archived_at && resolveSessionContinueMode(s) === "terminal").map(s => s.id));

  return (
    <DetailList label="会话列表" compact>
      {sessions.length === 0 && <div className="detail-empty">{emptyText}</div>}
      {visibleSessions.map((s) => (
        <DetailListRow key={s.id}
          icon={<span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>}
          title={sessionDisplayTitle(s.title)}
          titleHint={s.title ?? `${UNTITLED_SESSION} · ${s.root_agent_session_id}`}
          meta={<span title={s.last_activity_at ?? s.started_at ?? undefined}>{timeAgo(s.last_activity_at ?? s.started_at)}</span>}
          onOpen={() => navigate({ view: "session", sessionId: s.id, initialTitle: sessionDisplayTitle(s.title), initialAgent: s.agent })}
          actions={
              <SessionResumeButton
                session={s}
                resumeState={resumeStates[s.id]}
                agentStatus={agentStatus}
                onResume={continueSessionDesktopWithToast}
                navigate={navigate}
                terminals={terminals}
              />
          }
        />
      ))}
    </DetailList>
  );
}
