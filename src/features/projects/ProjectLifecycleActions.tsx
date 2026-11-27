import DialogActions from "../../components/DialogActions";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api } from "../../api";
import { notifyDataChanged, type Route } from "../../app/routes";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { Project, ProjectPurpose } from "../../types";
import { clearProjectCardsCache } from "./projectCardsCache";
import { clearProjectDetailCache } from "./projectDetailCache";

export type LifecycleProject = Pick<Project, "id" | "name" | "visibility"> & {
  purpose?: ProjectPurpose | null;
  is_agent?: boolean;
  session_count: number;
};

function canDeleteProject(project: LifecycleProject) {
  return project.visibility === "archived" && !project.purpose && project.session_count === 0;
}

export function ProjectDeleteModal({ names, busy, onClose, onConfirm, removesAgentDirectories, label = "项目" }: {
  names: string[];
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
  removesAgentDirectories?: boolean;
  label?: "项目" | "智能体项目";
}) {
  return (
    <Modal title={`永久删除${label}？`} onClose={() => { if (!busy) onClose(); }}>
      <p>确定永久删除 {names.length === 1 ? `「${names[0]}」` : `${names.length} 个${label}`}？</p>
      <p className="muted small">项目摘要和目录登记会被删除，任务关联会解除，任务本身会保留。此操作无法撤销。</p>
      {removesAgentDirectories
        ? <p className="muted small">智能体设置、信箱、档案目录及其中的文件也会永久删除。</p>
        : <p className="muted small">磁盘目录和文件会保留。</p>}
      <DialogActions>
        <button className="btn" disabled={busy} onClick={onClose}>取消</button>
        <button className="btn danger" disabled={busy} onClick={onConfirm}>{busy ? "删除中…" : "永久删除"}</button>
      </DialogActions>
    </Modal>
  );
}

export function invalidateProjectLifecycleCaches() {
  clearProjectCardsCache();
  clearProjectDetailCache();
}

export function BulkProjectDeleteAction({ projects, label = "项目", replacement }: {
  projects: readonly LifecycleProject[];
  label?: "项目" | "智能体项目";
  replacement?: Route;
}) {
  const [targets, setTargets] = useState<LifecycleProject[] | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const deletable = projects.filter(canDeleteProject);

  const remove = async () => {
    if (!targets?.length || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    let deleted = 0;
    const warnings: string[] = [];
    for (const project of targets) {
      try {
        const result = replacement
          ? await api.deleteProjectPermanently(project.id, replacement)
          : await api.deleteProjectPermanently(project.id);
        warnings.push(...result.warnings);
        deleted += 1;
      } catch (error) {
        console.error(error);
      }
    }
    inFlight.current = false;
    setBusy(false);
    setTargets(null);
    invalidateProjectLifecycleCaches();
    notifyDataChanged(["projects"]);
    if (warnings.length) showToast(warnings.join("。"));
    const completion = warnings.length ? `已移除 ${deleted} 个${label}登记` : `已永久删除 ${deleted} 个${label}`;
    showToast(deleted < targets.length ? `${completion}，${targets.length - deleted} 个删除失败` : completion);
  };

  return <>
    <button type="button" className="btn ghost icon-button danger" aria-label="删除全部"
      title={`删除全部可删除的已归档${label}`}
      disabled={!deletable.length || busy} onClick={() => setTargets(deletable)}><Icon name="trash" /></button>
    {targets && createPortal(<ProjectDeleteModal names={targets.map(project => project.name)}
      removesAgentDirectories={targets.some(project => project.is_agent)}
      label={label} busy={busy} onClose={() => setTargets(null)} onConfirm={() => void remove()} />, document.body)}
  </>;
}

export default function ProjectLifecycleActions({ project, onChanged, onDeleted, onRequestDelete, replacement }: {
  project: LifecycleProject;
  onChanged: () => void;
  onDeleted?: () => void;
  onRequestDelete?: (project: LifecycleProject) => void;
  replacement?: Route;
}) {
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const archived = project.visibility === "archived";
  const perform = async (remove: boolean) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      let warnings: string[] = [];
      if (remove) {
        const result = await api.deleteProjectPermanently(project.id, replacement);
        warnings = result.warnings;
      } else if (archived) await api.restoreProject(project.id);
      else await api.archiveProject(project.id);
      invalidateProjectLifecycleCaches();
      setConfirmDelete(false);
      if (mounted.current) {
        if (remove && onDeleted) onDeleted();
        else onChanged();
      }
      notifyDataChanged(["projects"]);
      showToast(warnings.length ? warnings.join("。") : remove ? "项目已永久删除"
        : archived ? "项目已恢复" : "项目已归档");
    } catch (e) {
      showToast(`操作失败：${String(e)}`);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };
  return (
    <>
      <button className="btn ghost icon-button" aria-label={archived ? "恢复项目" : "归档项目"}
        title={archived ? "恢复项目" : "归档项目"} disabled={busy} onClick={() => void perform(false)}>
        <Icon name={archived ? "unarchive" : "archive"} />
      </button>
      {archived && !project.purpose && (
        <button className="btn ghost icon-button danger" aria-label="永久删除项目"
          title={project.session_count > 0 ? "项目仍有关联会话，不能永久删除" : "永久删除项目"}
          disabled={busy || !canDeleteProject(project)} onClick={() => {
            if (onRequestDelete) onRequestDelete(project);
            else setConfirmDelete(true);
          }}>
          <Icon name="trash" />
        </button>
      )}
      {!onRequestDelete && confirmDelete && <ProjectDeleteModal names={[project.name]} busy={busy}
        removesAgentDirectories={project.is_agent}
        onClose={() => setConfirmDelete(false)} onConfirm={() => void perform(true)} />}
    </>
  );
}
