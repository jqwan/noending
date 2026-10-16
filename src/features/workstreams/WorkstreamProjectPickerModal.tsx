import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import type { ProjectCardData } from "../../types";

export default function WorkstreamProjectPickerModal({
  initialSelectedIds,
  onClose,
  onConfirm,
  title = "选择关联项目",
  projects: passedProjects,
  onProjectsChange,
}: {
  initialSelectedIds: string[];
  onClose: () => void;
  onConfirm: (selectedIds: string[]) => void | Promise<void>;
  title?: string;
  projects?: ProjectCardData[];
  onProjectsChange?: (projects: ProjectCardData[]) => void;
}) {
  const [selected, setSelected] = useState<string[]>(initialSelectedIds);
  const [projects, setProjects] = useState<ProjectCardData[]>(passedProjects ?? []);
  const [loading, setLoading] = useState(!passedProjects || passedProjects.length === 0);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);

  useEffect(() => {
    if (passedProjects && passedProjects.length > 0) {
      setProjects(passedProjects);
      setLoading(false);
      return;
    }
    let active = true;
    setLoading(true);
    api.listProjectCards()
      .then((p) => {
        if (active) {
          setProjects(p);
          onProjectsChange?.(p);
          setLoading(false);
        }
      })
      .catch((e) => {
        if (active) {
          setError(String(e));
          setLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [passedProjects, onProjectsChange]);

  const sortedProjects = useMemo(() => {
    return [...projects].sort((a, b) => {
      const aTime = a.last_activity_at || a.updated_at || a.created_at || "";
      const bTime = b.last_activity_at || b.updated_at || b.created_at || "";
      const cmp = bTime.localeCompare(aTime);
      return cmp !== 0 ? cmp : a.name.localeCompare(b.name);
    });
  }, [projects]);

  const filteredProjects = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return sortedProjects;
    return sortedProjects.filter((p) => p.name.toLowerCase().includes(q));
  }, [sortedProjects, search]);

  const browse = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      const path = await open({ directory: true, multiple: false, title: "选择项目目录" });
      if (!path) return;
      const result = await api.addProjectPath(Array.isArray(path) ? path[0] : path);
      const nextProjects = await api.listProjectCards();
      setProjects(nextProjects);
      onProjectsChange?.(nextProjects);
      setSelected((ids) => (ids.includes(result.project_id) ? ids : [...ids, result.project_id]));
    } catch (e) {
      setError(String(e));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  const handleConfirm = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      await onConfirm(selected);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  return (
    <Modal title={title} onClose={() => { if (!busy) onClose(); }}>
      <div className="workstream-project-picker-modal">
        <input
          type="text"
          className="workstream-project-search-input"
          placeholder="搜索项目…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          disabled={busy}
          autoFocus
        />

        <div className="workstream-project-modal-list">
          {loading ? (
            <div className="workstream-project-empty muted small">正在加载项目…</div>
          ) : filteredProjects.length === 0 ? (
            <div className="workstream-project-empty muted small">
              {projects.length === 0 ? "暂无项目" : "无匹配项目"}
            </div>
          ) : (
            filteredProjects.map((p) => {
              const isChecked = selected.includes(p.id);
              return (
                <label
                  key={p.id}
                  className={`workstream-project-item${isChecked ? " checked" : ""}`}
                  title={p.name}
                >
                  <input
                    type="checkbox"
                    checked={isChecked}
                    disabled={busy}
                    aria-label={p.name}
                    onChange={() => {
                      setSelected((ids) =>
                        isChecked ? ids.filter((id) => id !== p.id) : [...ids, p.id]
                      );
                    }}
                  />
                  <span className="workstream-project-item-icon">
                    <Icon name="project" />
                  </span>
                  <span className="workstream-project-item-name truncate">
                    {p.name}
                  </span>
                  {p.visibility === "archived" && (
                    <span
                      className="badge muted small"
                      style={{ flexShrink: 0, fontSize: 11, padding: "1px 6px" }}
                    >
                      已归档
                    </span>
                  )}
                </label>
              );
            })
          )}
        </div>

        <button
          type="button"
          className="btn ghost small"
          style={{ alignSelf: "flex-start", display: "inline-flex", alignItems: "center", gap: 6 }}
          disabled={busy}
          onClick={() => void browse()}
        >
          <Icon name="plus" />
          <span>新增项目</span>
        </button>

        {error && <p role="alert" className="warning" style={{ margin: 0 }}>{error}</p>}

        <div
          className="modal-actions row between"
          style={{
            marginTop: 8,
            paddingTop: 12,
            borderTop: "1px solid var(--border-subtle)",
          }}
        >
          <button type="button" className="btn" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn primary" disabled={busy} onClick={() => void handleConfirm()}>
            {busy ? "保存中…" : "确定"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
