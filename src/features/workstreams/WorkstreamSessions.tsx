import { useState } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import ResumeSessionModal from "../sessions/ResumeSessionModal";
import { sessionDisplayTitle, UNTITLED_SESSION } from "../sessions/SessionTable";
import { AGENT_LABELS, type Session } from "../../types";
import type { Route } from "../../app/routes";

/**
 * Workstream 的 Sessions 段落（方案 §14）：这一页的主角。
 *
 * Workstream = 用户显式组织的一组持续相关 Sessions，所以这里只回答
 * 「有哪些 Session / 继续哪个 / 再来一个」。每行的「继续」挂载 Resume 的
 * 同一个 Modal（§8.1.1 冻结契约），由它走 prepare → 状态指纹 → launch_prepared；
 * 本页不再直接调用 launcher。
 */
export default function WorkstreamSessions({ sessions, navigate, onNewSession, allowActions = true }: {
  sessions: Session[];
  navigate: (r: Route) => void;
  onNewSession?: () => void;
  allowActions?: boolean;
}) {
  const [resumeId, setResumeId] = useState<string | null>(null);

  const sorted = [...sessions].sort((a, b) =>
    (b.last_activity_at ?? b.started_at ?? "").localeCompare(
      a.last_activity_at ?? a.started_at ?? "",
    ),
  );

  return (
    <section className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>会话</div>
        {allowActions && onNewSession && (
          <button className="btn ghost icon-button" title="新建会话" aria-label="新建会话"
            onClick={onNewSession}>
            <Icon name="plus" />
          </button>
        )}
      </div>

      {sorted.length === 0 && (
        <div className="l1-none">还没有会话关联到这里。</div>
      )}

      {sorted.map((s) => (
        <div className="rail-row" role="link" tabIndex={0} onKeyDown={e => { if (e.key === "Enter" && e.target === e.currentTarget) navigate({ view: "session", sessionId: s.id }); }} key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
          <div className="rail-main">
            <div className="rail-title" title={s.title ?? `${UNTITLED_SESSION} · ${s.agent_session_id}`}>
              {sessionDisplayTitle(s.title)}
            </div>
            <div className="rail-sub">{timeAgo(s.last_activity_at ?? s.started_at)}</div>
          </div>
          {allowActions && (
            <button className="btn small"
              title={`继续 ${AGENT_LABELS[s.agent]} 会话`}
              onClick={(e) => { e.stopPropagation(); setResumeId(s.id); }}>
              继续
            </button>
          )}
        </div>
      ))}

      {resumeId && (
        <ResumeSessionModal sessionId={resumeId} onClose={() => setResumeId(null)} />
      )}
    </section>
  );
}
