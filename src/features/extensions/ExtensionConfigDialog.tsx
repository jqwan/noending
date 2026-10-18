import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { ExtensionLocation, ExtensionResource, ProjectCardData } from "../../types";
import { EXTENSION_AGENTS } from "./extensionAgents";
import { locationSupportsAgent, under } from "./extensionPresentation";
import "./extensions.css";

function configuredIds(projects: ProjectCardData[], resource: ExtensionResource, locations: ExtensionLocation[]) {
  const entries = resource.entry_paths.filter((_, index) =>
    locations.find(location => location.id === resource.locations[index])?.scope === "project");
  return new Set(projects
    .filter(project => project.search_paths.some(root => entries.some(entry => under(entry, root))))
    .map(project => project.id));
}

const usable = (project: ProjectCardData) => project.path_count > project.missing_path_count;

/** Scope set editor: all projects selected means user level, otherwise project level. */
export default function ExtensionConfigDialog({ resource, locations, onClose, onDone }: {
  resource: ExtensionResource; locations: ExtensionLocation[];
  onClose: () => void; onDone: () => void;
}) {
  const [projects, setProjects] = useState<ProjectCardData[] | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [master, setMaster] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const userEntries = resource.locations.flatMap(id => {
    const location = locations.find(item => item.id === id);
    return location?.scope === "user" ? [{ location }] : [];
  });
  const userLevel = userEntries.length > 0;
  const userFull = userLevel && EXTENSION_AGENTS.every(agent => userEntries.some(({ location }) =>
    resource.kind === "mcp" ? location.agent === agent : locationSupportsAgent(location, agent)));

  useEffect(() => {
    let live = true;
    void api.listProjectCards().then(rows => {
      if (!live) return;
      setProjects(rows);
      setSelected(userLevel ? rows.filter(usable).map(project => project.id) : [...configuredIds(rows, resource, locations)]);
      setMaster(userLevel);
    }).catch(e => {
      if (live) { setProjects([]); setError(String(e)); }
    });
    return () => { live = false; };
  }, [resource, locations, userLevel]);

  const usableIds = useMemo(() => (projects ?? []).filter(usable).map(project => project.id), [projects]);
  const configured = useMemo(() => configuredIds(projects ?? [], resource, locations), [projects, resource, locations]);
  const allSelected = usableIds.length > 0 && usableIds.every(id => selected.includes(id));
  const masterChecked = master || allSelected;
  const changed = masterChecked !== userLevel
    || (masterChecked && !userFull)
    || (!masterChecked && (selected.length !== configured.size || selected.some(id => !configured.has(id))));
  const submit = async () => {
    if (busy || !changed) return;
    setBusy(true); setError("");
    try {
      const messages: string[] = [];
      if (masterChecked) {
        messages.push(resource.kind === "mcp"
          ? (await api.enableMcpDefinitionUser(resource.id)).message
          : (await api.enableExtensionSkillAll(resource.id)).message);
      } else {
        if (userLevel) {
          messages.push(resource.kind === "mcp"
            ? (await api.disableMcpDefinitionUser(resource.id)).message
            : (await api.disableExtensionSkillAll(resource.id)).message);
        }
        const toEnable = selected.filter(id => !configured.has(id));
        const toDisable = [...configured].filter(id => !selected.includes(id));
        if (toEnable.length) {
          messages.push(resource.kind === "mcp"
            ? (await api.enableMcpDefinitionProjects(toEnable, resource.id)).message
            : (await api.enableExtensionSkillProjects(toEnable, resource.id)).message);
        }
        if (toDisable.length) {
          messages.push(resource.kind === "mcp"
            ? (await api.disableMcpDefinitionProjects(toDisable, resource.id)).message
            : (await api.disableExtensionSkillProjects(toDisable, resource.id)).message);
        }
      }
      showToast(messages.join("；") || "配置未变化");
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return <Modal title="配置扩展" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted">全选代表用户级配置，对所有项目生效；否则只写入勾选项目的项目级配置。</p>
    {userLevel && !masterChecked && <p className="small muted">保存后停用用户级，改为所选项目的项目级配置。</p>}
    {projects === null
      ? <p className="small muted" role="status">正在读取项目…</p>
      : <>
        <div className="extension-project-options">
          <label className="extension-check extension-project-option">
            <input type="checkbox" disabled={busy} checked={masterChecked}
              onChange={e => { setMaster(e.target.checked); setSelected(e.target.checked ? usableIds : []); }} />
            <span className="extension-project-name">全选</span>
            <span className="small muted">用户级 · 所有项目</span>
          </label>
          {projects.map(project => {
            const already = configured.has(project.id);
            const available = usable(project);
            return <label className="extension-check extension-project-option" key={project.id}>
              <input type="checkbox" disabled={busy || (!available && !already)} checked={masterChecked || selected.includes(project.id)}
                onChange={e => {
                  setMaster(false);
                  setSelected(current => e.target.checked ? [...current, project.id] : current.filter(id => id !== project.id));
                }} />
              <span className="extension-project-name">{project.name}</span>
              {project.visibility === "archived" && <span className="badge">已归档</span>}
              {already ? <span className="small muted">已配置</span> : !available && <span className="small muted">没有可用目录</span>}
            </label>;
          })}
        </div>
        {!projects.length && <p className="small muted">还没有项目；仍可选择全选以启用用户级。</p>}
      </>}
    {error && <div role="alert" className="extension-error">{error}</div>}
    <div className="row extension-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || !changed} onClick={() => void submit()}>{busy ? "保存中…" : "保存"}</button>
    </div>
  </Modal>;
}
