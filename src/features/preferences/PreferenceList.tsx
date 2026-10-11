import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import type { Preference } from "../../types";

/** Users can manage every item, including locked ones. No history is retained. */
export default function PreferenceList({ projectId }: { projectId: string | null }) {
  const title = projectId ? "项目偏好 / 准则" : "用户偏好 / 准则";
  const [items, setItems] = useState<Preference[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const active = useRef(true);
  const request = useRef(0);
  const [editing, setEditing] = useState<{ id: string | null; content: string; locked: boolean } | null>(null);
  const [editError, setEditError] = useState("");
  const refresh = useCallback(() => {
    const seq = ++request.current;
    api.listPreferences(projectId).then(next => {
      if (!active.current || seq !== request.current) return;
      setItems(next); setLoaded(true); setError("");
    }).catch(e => { if (active.current && seq === request.current) setError(String(e)); });
  }, [projectId]);
  useEffect(() => {
    active.current = true;
    refresh();
    window.addEventListener("noending:preferences", refresh);
    return () => { active.current = false; ++request.current; window.removeEventListener("noending:preferences", refresh); };
  }, [refresh]);
  useRefreshSignal(refresh);
  const mutate = async (action: () => Promise<unknown>, inEditor = false) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError(""); setEditError("");
    try {
      await action();
      if (!active.current) return;
      if (inEditor) setEditing(null);
      refresh();
      showToast("偏好已保存");
    } catch (e) {
      if (active.current) (inEditor ? setEditError : setError)(String(e));
    } finally {
      busyRef.current = false;
      if (active.current) setBusy(false);
    }
  };
  return <section className="preference-section" aria-label={title}>
    <div className="rail-head">
      <h3>{title}</h3>
      <button className="btn small ghost" disabled={busy} onClick={() => { setEditing({ id: null, content: "", locked: false }); setEditError(""); }}>新增</button>
    </div>
    <p className="muted small">随摘要提取更新；锁定仅禁止 Agent 修改或删除，您仍可编辑和删除。</p>
    {items.map(item => <div className="preference-row" key={item.id}>
      <div className="grow"><span className="preference-content">{item.content}</span>{item.locked && <span className="badge">已锁定</span>}</div>
      <div className="actions">
        <button className="btn small ghost" disabled={busy} onClick={() => mutate(() => api.savePreference(item.id, projectId, item.content, !item.locked))}>{item.locked ? "解锁" : "锁定"}</button>
        <button className="btn small ghost icon-button" title="编辑偏好" aria-label={`编辑偏好：${item.content}`} disabled={busy} onClick={() => { setEditing(item); setEditError(""); }}><Icon name="edit" /></button>
        <button className="btn small ghost icon-button" title="删除偏好" aria-label={`删除偏好：${item.content}`} disabled={busy} onClick={() => mutate(() => api.deletePreference(item.id))}><Icon name="trash" /></button>
      </div>
    </div>)}
    {loaded && items.length === 0 && <p className="muted small">暂无偏好或准则。</p>}
    {error && <div role="alert"><p>{error}</p><button className="btn small ghost" disabled={busy} onClick={refresh}>重试</button></div>}
    {editing && <Modal title={editing.id ? "编辑偏好" : "新增偏好"} onClose={() => { if (!busyRef.current) setEditing(null); }}>
      <label className="field"><span>偏好 / 准则</span><textarea autoFocus value={editing.content} disabled={busy} onChange={e => setEditing({ ...editing, content: e.target.value })} /></label>
      <label className="row small" style={{ gap: 8 }}><input type="checkbox" checked={editing.locked} disabled={busy} onChange={e => setEditing({ ...editing, locked: e.target.checked })} />锁定，禁止 Agent 修改或删除</label>
      {editError && <p role="alert" className="danger">{editError}</p>}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
        <button className="btn" disabled={busy} onClick={() => setEditing(null)}>取消</button>
        <button className="btn primary" disabled={busy || !editing.content.trim()} onClick={() => mutate(() => api.savePreference(editing.id, projectId, editing.content, editing.locked), true)}>{busy ? "保存中…" : "保存"}</button>
      </div>
    </Modal>}
  </section>;
}
