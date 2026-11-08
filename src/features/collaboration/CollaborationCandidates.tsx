import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import type { CollaborationCandidate, CollaborationStartedMember, SessionListItem } from "../../types";
import { WAKE_LABEL, defaultWakeMethod } from "./collaborationState";
import { collaborationCandidatesResource } from "./collaborationResources";
import { DetailLoading } from "../../components/DetailSection";
import type { Route } from "../../app/routes";
import AgentIcon from "../../components/AgentIcon";
import SessionMiniList from "../sessions/SessionMiniList";

const PAGE = 20;

/** 候选会话的展示名：优先用后端给的名字，其次本地任务会话，最后退回 ID。 */
function candidateName(candidate: CollaborationCandidate, sessions: SessionListItem[]): string {
  if (candidate.title) return candidate.title;
  const local = sessions.find(s => s.id === candidate.session_id);
  if (local?.title) return local.title;
  return candidate.session_id.slice(0, 8);
}

/**
 * 本次成员选择。候选只来自该任务关联、未删除且可继续的会话；
 * 不可加入的项照样列出原因而不是悄悄消失——用户需要看到为什么进不来。
 * 至少两个会话成员才能开启；用户本人不计入这个人数。
 */
export default function CollaborationCandidates({
  taskId,
  sessions,
  selected,
  coordinator,
  onChange,
  disabled = false,
  navigate,
}: {
  taskId: string;
  sessions: SessionListItem[];
  selected: CollaborationStartedMember[];
  coordinator: string | null;
  onChange: (next: { selected: CollaborationStartedMember[]; coordinator: string | null }) => void;
  disabled?: boolean;
  navigate: (route: Route) => void;
}) {
  const page = collaborationCandidatesResource.useValue(taskId);
  const items = page?.items ?? [];
  const cursor = page?.next_cursor ?? null;
  const loading = page === undefined;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const token = useRef(0);
  const sessionKey = JSON.stringify(sessions.map(session => session.id).sort());

  const load = useCallback(async (next: string | null, append: boolean) => {
    const current = ++token.current;
    setBusy(true);
    try {
      const page = append ? await api.collaborationCandidates(taskId, next, PAGE) : await collaborationCandidatesResource.refresh(taskId);
      if (token.current !== current) return;
      if (append) {
        const old = collaborationCandidatesResource.get(taskId)?.items ?? [];
        collaborationCandidatesResource.set(taskId, { ...page, items: [...old, ...page.items.filter(c => !old.some(o => o.session_id === c.session_id))] });
      }
      setError("");
    } catch (e) {
      if (token.current === current) setError(String(e));
    } finally {
      if (token.current === current) setBusy(false);
    }
  }, [taskId]);

  useEffect(() => {
    void load(null, false);
    return () => { token.current += 1; };
  }, [load, sessionKey]);

  const toggle = (candidate: CollaborationCandidate, checked: boolean) => {
    if (candidate.unavailable_reason) return;
    if (checked) {
      const mode = defaultWakeMethod(candidate.wake_modes);
      if (!mode) return;
      onChange({ selected: [...selected, { session_id: candidate.session_id, wake_method: mode }], coordinator });
      return;
    }
    // 移出成员时一并让出负责人，避免出现「负责人不在成员内」。
    onChange({
      selected: selected.filter(m => m.session_id !== candidate.session_id),
      coordinator: coordinator === candidate.session_id ? null : coordinator,
    });
  };

  const setWake = (sessionId: string, mode: CollaborationStartedMember["wake_method"]) => {
    onChange({ selected: selected.map(m => (m.session_id === sessionId ? { ...m, wake_method: mode } : m)), coordinator });
  };

  const chosen = selected.length;
  return (
    <div className="collab-candidates">
      <p className="small muted" style={{ margin: 0 }}>
        勾选参与本次协作的会话（至少 2 个）。
      </p>

      {loading ? <>
        {!error && <DetailLoading>正在读取候选会话…</DetailLoading>}
        {sessions.length > 0 && <SessionMiniList sessions={sessions} emptyText="" navigate={navigate} />}
      </> : (
      items.length === 0 ? (
        <p className="small muted" style={{ margin: 0 }}>这项任务还没有可参与的会话。</p>
      ) : (
        <ul className="collab-candidate-list">
          {items.map(candidate => {
            const chosenMember = selected.find(m => m.session_id === candidate.session_id);
            const blocked = candidate.unavailable_reason ?? (candidate.wake_modes.length === 0 ? "该会话没有可用的唤醒方式" : null);
            return (
              <li key={candidate.session_id} className={`collab-candidate${blocked ? " blocked" : ""}`}>
                <div className="collab-candidate-main">
                  <input
                    type="checkbox"
                    checked={!!chosenMember}
                    disabled={disabled || !!blocked}
                    aria-label={candidateName(candidate, sessions)}
                    onChange={e => toggle(candidate, e.target.checked)}
                  />
                  {candidate.agent && <AgentIcon agent={candidate.agent} />}
                  <button type="button" className="link collab-candidate-name truncate" title={candidateName(candidate, sessions)}
                    onClick={() => navigate({ view: "session", sessionId: candidate.session_id, initialTitle: candidateName(candidate, sessions), initialAgent: candidate.agent ?? undefined })}>{candidateName(candidate, sessions)}</button>
                  {blocked && <span className="badge muted small" title={blocked}>{blocked}</span>}
                </div>
                <div className="collab-candidate-wake" role="group" aria-label={`${candidateName(candidate, sessions)} 的唤醒方式`} aria-hidden={!chosenMember}>
                  {candidate.wake_modes.map(mode => (
                    <button key={mode} type="button" className="btn ghost icon-button small"
                      aria-label={WAKE_LABEL[mode]} title={WAKE_LABEL[mode]} aria-pressed={chosenMember?.wake_method === mode}
                      disabled={!chosenMember || disabled || candidate.wake_modes.length === 1}
                      onClick={() => setWake(candidate.session_id, mode)}><Icon name={mode} /></button>
                  ))}
                </div>
              </li>
            );
          })}
        </ul>
      ))}

      {cursor && (
        <button type="button" className="btn small ghost" disabled={busy || disabled} onClick={() => void load(cursor, true)}>
          {busy ? "加载中…" : "加载更多候选"}
        </button>
      )}

      {chosen > 0 && chosen < 2 && (
        <p className="small warning" role="status">还需要再选 1 个会话才能开启协作。</p>
      )}
      {error && <p role="alert" className="small warning">{error}</p>}

    </div>
  );
}
