import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import WorkstreamProjectPickerModal from "./WorkstreamProjectPickerModal";
import type { ProjectCardData, Workstream, WorkstreamProject } from "../../types";

/** Projects are explicit memberships; editing a task never expands directories. */
export default function WorkstreamFormModal({ onClose,onCreated,onSaved,workstream,projects: initialProjects=[],initialProjectId }: {
  onClose: () => void; onCreated?: (w:Workstream)=>void; onSaved?:()=>void;
  workstream?:Workstream; projects?:WorkstreamProject[]; initialProjectId?:string;
}) {
  const [title,setTitle]=useState(workstream?.title ?? "");
  const [description,setDescription]=useState(workstream?.description ?? "");
  const [projects,setProjects]=useState<ProjectCardData[]>([]);
  const [selected,setSelected]=useState<string[]>(()=>workstream ? initialProjects.map(p=>p.id) : initialProjectId && initialProjectId!=="none" ? [initialProjectId]:[]);
  const [pickerModalOpen, setPickerModalOpen] = useState(false);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState("");
  const pending=useRef(false);
  useEffect(() => {
    let active = true;
    api.listProjectCards()
      .then((p) => {
        if (active) setProjects(p);
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!workstream && projects.length > 0) {
      setSelected((ids) => ids.filter((id) => projects.some((p) => p.id === id)));
    }
  }, [projects, workstream]);

  const save = async () => {
    if (pending.current || !title.trim()) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      if (workstream) {
        await api.updateWorkstream({ ...workstream, title: title.trim(), description: description.trim() });
        await api.setWorkstreamProjects(workstream.id, selected);
        onSaved?.();
      } else {
        const created = await api.createWorkstream(title.trim(), description.trim(), selected);
        onCreated?.(created);
      }
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  return (
    <Modal title={workstream ? "编辑任务" : "新建任务"} onClose={() => { if (!busy) onClose(); }}>
      <form onSubmit={(e) => { e.preventDefault(); void save(); }} style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <label className="field">
          <span>标题</span>
          <input
            type="text"
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            disabled={busy}
            placeholder="例如：重构前端路由与组件库..."
          />
        </label>
        <label className="field">
          <span>描述</span>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={busy}
            rows={3}
            placeholder="简要说明任务目标与背景（可选）..."
          />
        </label>
        <div className="field workstream-projects-field">
          <div className="row between" style={{ marginBottom: 6, alignItems: "center" }}>
            <div className="row" style={{ alignItems: "center", gap: 6 }}>
              <span>关联项目</span>
              <button
                type="button"
                className="btn ghost icon-button small"
                title="添加关联项目"
                aria-label="添加关联项目"
                disabled={busy}
                onClick={() => setPickerModalOpen(true)}
              >
                <Icon name="plus" />
              </button>
            </div>
            <span className="muted small">可选，可多选</span>
          </div>

          {selected.length > 0 ? (
            <div className="workstream-project-chips">
              {selected.map((id) => {
                const project = projects.find((p) => p.id === id) ?? initialProjects.find((p) => p.id === id);
                return (
                  <div className="workstream-project-chip" key={id}>
                    <Icon name="project" />
                    <span>{project?.name ?? id}</span>
                    <button
                      type="button"
                      className="workstream-project-chip-remove"
                      disabled={busy}
                      aria-label={`移除项目 ${project?.name ?? id}`}
                      onClick={() => setSelected((ids) => ids.filter((p) => p !== id))}
                    >
                      <Icon name="close" />
                    </button>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="muted small" style={{ marginTop: 2, marginBottom: 4 }}>
              暂未关联项目
            </div>
          )}
        </div>
        {error && <p role="alert" className="warning" style={{ margin: "4px 0 0" }}>{error}</p>}
        {pickerModalOpen && (
          <WorkstreamProjectPickerModal
            initialSelectedIds={selected}
            projects={projects}
            onProjectsChange={(next) => setProjects(next)}
            onClose={() => setPickerModalOpen(false)}
            onConfirm={(nextSelected) => {
              setSelected(nextSelected);
            }}
          />
        )}
        <div
          className="modal-actions row between"
          style={{
            marginTop: 12,
            paddingTop: 16,
            borderTop: "1px solid var(--border-subtle)",
          }}
        >
          <div />
          <div className="row" style={{ gap: 10 }}>
            <button type="button" className="btn" disabled={busy} onClick={onClose}>
              取消
            </button>
            <button type="submit" className="btn primary" disabled={busy || !title.trim()}>
              {busy ? "保存中…" : workstream ? "保存" : "创建"}
            </button>
          </div>
        </div>
      </form>
    </Modal>
  );
}
