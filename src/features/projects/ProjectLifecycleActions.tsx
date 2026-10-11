import { useRef, useState } from "react";
import { api } from "../../api";
import { EVT_SYNCED } from "../../app/routes";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { Project, ProjectPurpose } from "../../types";

type LifecycleProject = Pick<Project, "id" | "name" | "visibility"> & {
  purpose?: ProjectPurpose | null;
  session_count: number;
};

export function canDeleteProject(project: LifecycleProject) {
  return project.visibility === "archived" && !project.purpose && project.session_count === 0;
}

export function ProjectDeleteModal({ names, busy, onClose, onConfirm }: {
  names: string[];
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal title="永久删除项目？" onClose={() => { if (!busy) onClose(); }}>
      <p>确定永久删除 {names.length === 1 ? `「${names[0]}」` : `${names.length} 个项目`}？</p>
      <p className="muted small">项目摘要、偏好和目录登记会被删除，所有任务关联会解除。任务和磁盘目录会保留。此操作无法撤销。</p>
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" disabled={busy} onClick={onClose}>取消</button>
        <button className="btn danger" disabled={busy} onClick={onConfirm}>{busy ? "删除中…" : "永久删除"}</button>
      </div>
    </Modal>
  );
}

export default function ProjectLifecycleActions({ project, onChanged, onDeleted }: {
  project: LifecycleProject;
  onChanged: () => void;
  onDeleted?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const archived = project.visibility === "archived";
  const perform = async (remove: boolean) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      if (remove) await api.deleteProjectPermanently(project.id);
      else if (archived) await api.restoreProject(project.id);
      else await api.archiveProject(project.id);
      setConfirmDelete(false);
      onChanged();
      if (remove) onDeleted?.();
      window.dispatchEvent(new Event(EVT_SYNCED));
      showToast(remove ? "项目已永久删除" : archived ? "项目已恢复" : "项目已归档");
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
          disabled={busy || !canDeleteProject(project)} onClick={() => setConfirmDelete(true)}>
          <Icon name="trash" />
        </button>
      )}
      {confirmDelete && <ProjectDeleteModal names={[project.name]} busy={busy}
        onClose={() => setConfirmDelete(false)} onConfirm={() => void perform(true)} />}
    </>
  );
}
