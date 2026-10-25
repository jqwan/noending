import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { useRefreshSignal } from "../../components/common";
import type { SummaryDocument, SummaryItem, TaskItem } from "../../types";
import SummaryContent, { SummaryControls, useSummaryExpansion } from "../sessions/SummaryContent";
import TaskItemEditor, { type ItemEditorState } from "./TaskItemEditor";

type Draft = { original: TaskItem | null; index: number | null; base: SummaryDocument; item: SummaryItem };
const documentOf = (items: TaskItem[]): SummaryDocument => ({ items: items.map(({ id, title, description, details }) => ({ id, title, description, details })) });

export default function TaskItems({ taskId }: { taskId: string }) {
  const [items, setItems] = useState<TaskItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const expansion = useSummaryExpansion(documentOf(items));
  const active = useRef(true);
  const writing = useRef(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const next = await api.listTaskItems(taskId);
      if (active.current && request === generation.current) setItems(next);
    } catch (failure) {
      if (active.current && request === generation.current) setError(`读取任务事项失败：${String(failure)}`);
    } finally {
      if (active.current && request === generation.current) setLoading(false);
    }
  }, [taskId]);
  useEffect(() => {
    active.current = true; void refresh();
    const focused = () => { void refresh(); };
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, 3000);
    window.addEventListener("focus", focused);
    return () => { active.current = false; ++generation.current; window.clearInterval(timer); window.removeEventListener("focus", focused); };
  }, [refresh]);
  useRefreshSignal(refresh);
  const mutate = async (action: () => Promise<unknown>) => {
    if (writing.current) return;
    writing.current = true; setBusy(true); setError("");
    try {
      await action();
      if (!active.current) return;
      setDraft(null);
      await refresh();
      window.dispatchEvent(new Event("noending:sync"));
    } catch (failure) {
      if (active.current) { setError(`操作失败：${String(failure)}。请核对最新事项后重试。`); void refresh(); }
    } finally { writing.current = false; if (active.current) setBusy(false); }
  };
  const start = (original: TaskItem | null) => {
    const base = documentOf(items);
    const index = original ? items.findIndex(item => item.id === original.id) : null;
    setDraft({ original, base, index, item: original ? structuredClone(base.items[index!]) : { id: "", title: "", description: "", details: [] } });
    setError("");
  };
  const stale = Boolean(draft?.original && items.find(item => item.id === draft.original?.id)?.revision !== draft.original.revision);
  const save = (remove = false) => {
    if (!draft || stale || busy) return;
    const previous = draft.original;
    if (remove) {
      if (previous) void mutate(() => api.deleteTaskItem(taskId, previous.id, previous.revision));
      return;
    }
    const title = draft.item.title.trim();
    if (!title || /[\r\n]/.test(title)) { setError("请填写非空单行事项标题。"); return; }
    const content = { title, description: draft.item.description.trim() };
    void mutate(() => previous
      ? api.updateTaskItem(taskId, previous.id, previous.revision, { ...content, details: draft.item.details.map(text => text.trim()).filter(Boolean) })
      : api.createTaskItem(taskId, content));
  };
  const editorState: ItemEditorState = {
    draft, saving: busy, available: !loading && !busy, stale, error,
    save, cancel: () => { if (!busy) { setDraft(null); setError(""); } },
    change: item => { if (!busy) setDraft(current => current ? { ...current, item } : current); },
  };
  const editor = draft && <TaskItemEditor editor={editorState} showDetails={Boolean(draft.original)} />;
  const actions = (summary: SummaryItem) => {
    const item = items.find(item => item.id === summary.id)!;
    return <>
      <button type="button" className={`btn ghost icon-button small task-item-status${item.completed ? " is-completed" : ""}`}
        title={item.completed ? "已完成，点击标记未完成" : "未完成，点击标记完成"}
        aria-label={`${item.completed ? "标记未完成" : "标记完成"}：${item.title}`} aria-pressed={item.completed}
        disabled={busy || Boolean(draft)} onClick={() => void mutate(() => api.updateTaskItem(taskId, item.id, item.revision, { completed: !item.completed }))}
      ><Icon name={item.completed ? "circleCheck" : "circle"} /></button>
      <button type="button" className="btn ghost icon-button small" title="编辑事项" aria-label={`编辑事项：${item.title}`} disabled={busy || Boolean(draft)} onClick={() => start(item)}><Icon name="edit" /></button>
    </>;
  };
  const ordered = [...items].sort((a, b) => Number(a.completed) - Number(b.completed));
  return <section className="card detail-section" aria-label="任务事项">
    <div className="row between"><h2 className="detail-section-title"><Icon name="tasks" /><span>任务事项（{items.length}）</span></h2><div className="row"><SummaryControls expansion={expansion} disabled={Boolean(draft)} /><button type="button" className="btn ghost icon-button small" aria-label="新增事项" title="新增事项" disabled={loading || busy || Boolean(draft)} onClick={() => start(null)}><Icon name="plus" /></button></div></div>
    {error && !draft && <div role="alert"><span>{error}</span><button type="button" className="btn ghost small" disabled={busy} onClick={() => { setError(""); void refresh(); }}>重新读取</button></div>}
    {loading ? <div className="small muted">读取任务事项…</div> : <>
      <div className="md-body"><SummaryContent
        expansion={expansion} actions={actions} editing={Boolean(draft)}
        renderEditor={item => draft?.original?.id === item.id ? editor : null}
      >{documentOf(ordered)}</SummaryContent></div>
      {items.length === 0 && !draft && <div className="small muted">暂无任务事项</div>}
      {draft && (!draft.original || !items.some(item => item.id === draft.original?.id)) && editor}
    </>}
  </section>;
}
