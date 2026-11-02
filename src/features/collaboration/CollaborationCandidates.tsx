import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import type { CollaborationCandidate, CollaborationStartedMember, SessionListItem } from "../../types";
import { WAKE_LABEL, defaultWakeMethod } from "./collaborationState";
import CollaborationCoordinator from "./CollaborationCoordinator";

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
}: {
  taskId: string;
  sessions: SessionListItem[];
  selected: CollaborationStartedMember[];
  coordinator: string | null;
  onChange: (next: { selected: CollaborationStartedMember[]; coordinator: string | null }) => void;
  disabled?: boolean;
}) {
  const [items, setItems] = useState<CollaborationCandidate[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const token = useRef(0);

  const load = useCallback(async (next: string | null, append: boolean) => {
    const current = ++token.current;
    setBusy(true);
    try {
      const page = await api.collaborationCandidates(taskId, next, PAGE);
      if (token.current !== current) return;
      setItems(old => (append ? [...old, ...page.items.filter(c => !old.some(o => o.session_id === c.session_id))] : page.items));
      setCursor(page.next_cursor);
      setError("");
    } catch (e) {
      if (token.current === current) setError(String(e));
    } finally {
      if (token.current === current) { setBusy(false); setLoading(false); }
    }
  }, [taskId]);

  useEffect(() => {
    setItems([]);
    setCursor(null);
    setLoading(true);
    void load(null, false);
    return () => { token.current += 1; };
  }, [load]);

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
        选择至少 2 个会话参与协作。
      </p>

      {loading ? (
        <p className="small muted" style={{ margin: 0 }}>正在读取候选会话…</p>
      ) : items.length === 0 ? (
        <p className="small muted" style={{ margin: 0 }}>这项任务还没有可参与的会话。</p>
      ) : (
        <ul className="collab-candidate-list">
          {items.map(candidate => {
            const chosenMember = selected.find(m => m.session_id === candidate.session_id);
            const blocked = candidate.unavailable_reason ?? (candidate.wake_modes.length === 0 ? "该会话没有可用的唤醒方式" : null);
            return (
              <li key={candidate.session_id} className={`collab-candidate${blocked ? " blocked" : ""}`}>
                <label className="collab-candidate-main">
                  <input
                    type="checkbox"
                    checked={!!chosenMember}
                    disabled={disabled || !!blocked}
                    aria-label={candidateName(candidate, sessions)}
                    onChange={e => toggle(candidate, e.target.checked)}
                  />
                  <span className="collab-candidate-name truncate">{candidateName(candidate, sessions)}</span>
                  {blocked && <span className="badge muted small" title={blocked}>{blocked}</span>}
                </label>
                {chosenMember && (
                  <div className="collab-candidate-wake" role="group" aria-label={`${candidateName(candidate, sessions)} 的唤醒方式`}>
                    {candidate.wake_modes.map(mode => (
                      <button key={mode} type="button" className="btn ghost icon-button small"
                        aria-label={WAKE_LABEL[mode]} title={WAKE_LABEL[mode]} aria-pressed={chosenMember.wake_method === mode}
                        disabled={disabled || candidate.wake_modes.length === 1}
                        onClick={() => setWake(candidate.session_id, mode)}><Icon name={mode} /></button>
                    ))}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {cursor && (
        <button type="button" className="btn small ghost" disabled={busy || disabled} onClick={() => void load(cursor, true)}>
          {busy ? "加载中…" : "加载更多候选"}
        </button>
      )}

      <CollaborationCoordinator value={coordinator} disabled={disabled}
        members={selected.map(member => {
          const candidate = items.find(c => c.session_id === member.session_id);
          return { id: member.session_id, title: candidate ? candidateName(candidate, sessions) : sessions.find(s => s.id === member.session_id)?.title ?? member.session_id.slice(0, 8) };
        })}
        onChange={id => onChange({ selected, coordinator: id })} />

      {chosen > 0 && chosen < 2 && (
        <p className="small warning" role="status">还需要再选 1 个会话才能开启协作。</p>
      )}
      {error && <p role="alert" className="small warning">{error}</p>}
      {chosen >= 2 && (
        <p className="small muted" style={{ margin: 0 }}>
          <Icon name="check" /> 已选 {chosen} 个成员。
        </p>
      )}
    </div>
  );
}
