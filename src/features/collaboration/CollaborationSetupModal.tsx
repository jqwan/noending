import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import CustomSelect from "../../components/CustomSelect";
import { DetailLoading } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import type { Route } from "../../app/routes";
import type { CollaborationCandidate, CollaborationCandidatePage, CollaborationStartedMember, WorkstreamProject } from "../../types";
import CollaborationCoordinator from "./CollaborationCoordinator";
import { WAKE_LABEL, defaultWakeMethod } from "./collaborationState";

const PAGE = 20;

export type DraftMember = CollaborationStartedMember & Pick<CollaborationCandidate, "title" | "project_id">;
export interface CollaborationSelection {
  selected: DraftMember[];
  coordinator: string | null;
}

function sessionName(session: Pick<CollaborationCandidate, "title" | "session_id">) {
  return session.title || `会话 ${session.session_id.slice(0, 8)}`;
}

export default function CollaborationSetupModal({
  taskId, projects, selection, firstMessage, navigate, onChange, onClose, onStart,
}: {
  taskId: string;
  projects: WorkstreamProject[];
  selection: CollaborationSelection;
  firstMessage: string | null;
  navigate: (route: Route) => void;
  onChange: (next: CollaborationSelection) => void;
  onClose: () => void;
  onStart: () => void;
}) {
  const [query, setQuery] = useState("");
  const [projectFilter, setProjectFilter] = useState("");
  const projectId = projects.some(project => project.id === projectFilter) ? projectFilter : null;
  const filter = query.trim();
  const scope = JSON.stringify([taskId, projectId, filter, projects.map(project => project.id).sort()]);
  const [result, setResult] = useState<{ scope: string; page: CollaborationCandidatePage } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const request = useRef(0);
  const page = result?.scope === scope ? result.page : null;
  const hasProjects = projects.length > 0;
  const projectNames = new Map(projects.map(project => [project.id, project.name]));

  useEffect(() => {
    const current = ++request.current;
    setError("");
    setBusy(hasProjects);
    if (!hasProjects) return;
    const timer = window.setTimeout(() => {
      api.collaborationCandidates(taskId, null, PAGE, projectId, filter || null).then(next => {
        if (request.current === current) setResult({ scope, page: next });
      }).catch(error => {
        if (request.current === current) setError(String(error));
      }).finally(() => {
        if (request.current === current) setBusy(false);
      });
    }, filter ? 180 : 0);
    return () => { window.clearTimeout(timer); request.current += 1; };
  }, [taskId, projectId, filter, scope, retry, hasProjects]);

  const loadMore = async () => {
    if (!page?.next_cursor || busy) return;
    const current = ++request.current;
    setBusy(true);
    setError("");
    try {
      const next = await api.collaborationCandidates(taskId, page.next_cursor, PAGE, projectId, filter || null);
      if (request.current !== current) return;
      const existing = new Set(page.items.map(item => item.session_id));
      setResult({ scope, page: { ...next, items: [...page.items, ...next.items.filter(item => !existing.has(item.session_id))] } });
    } catch (error) {
      if (request.current === current) setError(String(error));
    } finally {
      if (request.current === current) setBusy(false);
    }
  };

  const { selected, coordinator } = selection;
  const remove = (sessionId: string) => onChange({
    selected: selected.filter(member => member.session_id !== sessionId),
    coordinator: coordinator === sessionId ? null : coordinator,
  });
  const add = (candidate: CollaborationCandidate) => {
    const wake = defaultWakeMethod(candidate.wake_modes);
    if (!wake || candidate.unavailable_reason) return;
    const { session_id, title, project_id } = candidate;
    onChange({ selected: [...selected, { session_id, title, project_id, wake_method: wake }], coordinator });
  };

  return (
    <Modal title="选择协作成员" wide onClose={onClose}>
      <div className="collab-setup-columns">
        <section className="collab-setup-pane" aria-labelledby="collab-available-heading">
          <h3 id="collab-available-heading">候选会话</h3>
          {hasProjects && <div className="collab-setup-filters">
            <input type="search" aria-label="搜索候选会话" placeholder="搜索标题或 Agent…" value={query}
              onChange={event => setQuery(event.target.value)} />
            {projects.length > 1 && <CustomSelect ariaLabel="筛选候选项目" value={projectId ?? ""}
              options={[{ value: "", label: "全部关联项目" }, ...projects.map(project => ({ value: project.id, label: project.name }))]}
              onChange={setProjectFilter} />}
          </div>}
          {!hasProjects ? <p className="muted small">请先为任务关联项目。</p>
            : !page && !error ? <DetailLoading>正在读取候选会话…</DetailLoading> : null}
          {page && !page.items.length && !error && !busy && <p className="muted small">
            {filter ? "没有匹配的会话。" : "所选项目没有会话。"}
          </p>}
          {page && page.items.length > 0 && <ul className="collab-candidate-list collab-setup-scroll" aria-label="候选会话列表" tabIndex={0}>
            {page.items.map(candidate => {
              const name = sessionName(candidate);
              const chosen = selected.find(member => member.session_id === candidate.session_id);
              const blocked = candidate.unavailable_reason ?? (candidate.wake_modes.length === 0 ? "没有可用的唤醒方式" : null);
              return (
                <li className="collab-candidate" key={candidate.session_id}>
                  <input type="checkbox" aria-label={name} checked={!!chosen} disabled={!!blocked && !chosen}
                    onChange={event => event.target.checked ? add(candidate) : remove(candidate.session_id)} />
                  {candidate.agent && <AgentIcon agent={candidate.agent} />}
                  <div className="collab-candidate-copy">
                    <button type="button" className="link truncate" title={name}
                      onClick={() => {
                        onClose();
                        navigate({ view: "session", sessionId: candidate.session_id, initialTitle: name, initialAgent: candidate.agent ?? undefined });
                      }}>{name}</button>
                    {candidate.project_id && <div className="small muted truncate">{projectNames.get(candidate.project_id)}</div>}
                    {blocked && <div className="small muted">{blocked}</div>}
                  </div>
                  {chosen && candidate.wake_modes.length > 1 && <div className="collab-candidate-wake" role="group" aria-label={`${name} 的唤醒方式`}>
                    {candidate.wake_modes.map(mode => (
                      <button key={mode} type="button" className="btn ghost icon-button small" aria-label={WAKE_LABEL[mode]}
                        title={WAKE_LABEL[mode]} aria-pressed={chosen.wake_method === mode}
                        onClick={() => onChange({ selected: selected.map(member => member === chosen ? { ...member, wake_method: mode } : member), coordinator })}>
                        <Icon name={mode} />
                      </button>
                    ))}
                  </div>}
                </li>
              );
            })}
          </ul>}
          {error && <p role="alert" className="warning small">{error}</p>}
          {page?.next_cursor ? <button type="button" className="btn small ghost" disabled={busy} onClick={() => void loadMore()}>
            {busy ? "加载中…" : error ? "重试加载更多" : "加载更多候选"}
          </button> : error && <button type="button" className="btn small" onClick={() => setRetry(value => value + 1)}>重试读取</button>}
        </section>
        <section className="collab-setup-pane" aria-labelledby="collab-selected-heading">
          <h3 id="collab-selected-heading">已选成员（{selected.length}）</h3>
          {selected.length ? <ul className="collab-candidate-list collab-setup-scroll" aria-label="已选成员列表" tabIndex={0}>
            {selected.map(member => (
              <li key={member.session_id} className="collab-setup-selected">
                <div className="collab-candidate-copy">
                  <span className="truncate" title={sessionName(member)}>{sessionName(member)}</span>
                  <span className="small muted">{WAKE_LABEL[member.wake_method]}</span>
                </div>
                <button type="button" className="btn ghost icon-button small" aria-label={`移除${sessionName(member)}`}
                  title="移除成员" onClick={() => remove(member.session_id)}><Icon name="close" /></button>
              </li>
            ))}
          </ul> : <p className="muted small">至少选择 2 个会话。</p>}
          <CollaborationCoordinator value={coordinator} disabled={false} members={selected.map(member => ({ id: member.session_id, title: sessionName(member) }))}
            onChange={sessionId => onChange({ selected, coordinator: sessionId })} />
        </section>
      </div>
      {firstMessage && <details className="collab-setup-message" open>
        <summary>随协作发送的首条消息</summary>
        <p className="small">{firstMessage}</p>
      </details>}
      <DialogActions>
        {selected.length === 1 && <span className="small muted collab-setup-hint" role="status">还需选择 1 个会话。</span>}
        <button type="button" className="btn" onClick={onClose}>取消</button>
        <button type="button" className="btn primary" disabled={selected.length < 2} onClick={onStart}>
          {firstMessage ? "发送并开启" : "开启协作"}
        </button>
      </DialogActions>
    </Modal>
  );
}
