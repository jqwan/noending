import React from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Session, type SessionBindingRow } from "../../types";

/**
 * Sessions 表格（整体设计方案 §38/§40）：Session 数量多，Table 优于 Card。
 * 点击行 → Session Detail；行内 Resume → 指定 Session 恢复。
 */
export default function SessionTable({ sessions, bindings, onOpen, onResume }: {
  sessions: Session[];
  bindings: Map<string, SessionBindingRow[]>;
  onOpen: (sessionId: string) => void;
  onResume: (sessionId: string) => void;
}) {
  return (
    <table className="session-table">
      <thead>
        <tr>
          <th style={{ width: 120 }}>Agent</th>
          <th>Session</th>
          <th style={{ width: 220 }}>Workstream</th>
          <th style={{ width: 100 }}>Last Active</th>
          <th style={{ width: 90 }}></th>
        </tr>
      </thead>
      <tbody>
        {sessions.map((s) => {
          const sBindings = bindings.get(s.id) ?? [];
          return (
            <tr key={s.id} onClick={() => onOpen(s.id)}>
              <td className="cell-agent">
                <AgentIcon agent={s.agent} />
                {AGENT_LABELS[s.agent]}
              </td>
              <td className="cell-title">{s.title ?? s.agent_session_id}</td>
              <td>{sBindings[0]?.workstream_title ?? "—"}</td>
              <td>{timeAgo(s.last_activity_at ?? s.started_at)}</td>
              <td onClick={(e) => e.stopPropagation()}>
                <button className="btn small" onClick={() => onResume(s.id)}>Resume</button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
