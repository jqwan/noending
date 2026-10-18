import { useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { ExtensionResource } from "../../types";
import "./extensions.css";

/** Additive project-level configuration for registered Skills and MCP definitions. */
export default function ProjectExtensionDialog({ projectId, resources, enabledIds, userEffectiveIds, onClose, onDone }: {
  projectId: string; resources: ExtensionResource[]; enabledIds: string[]; userEffectiveIds: string[];
  onClose: () => void; onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ordered = [...resources].sort((a, b) => a.name.localeCompare(b.name, "zh-Hans"));
  const removableIds = enabledIds.filter(id => !userEffectiveIds.includes(id));
  const [selected, setSelected] = useState<string[]>(removableIds);
  const toEnable = selected.filter(id => !enabledIds.includes(id));
  const toDisable = removableIds.filter(id => !selected.includes(id));
  const changed = toEnable.length > 0 || toDisable.length > 0;

  const submit = async () => {
    if (!changed || busy) return;
    setBusy(true); setError("");
    try {
      for (const id of [...toEnable, ...toDisable]) {
        const row = ordered.find(item => item.id === id);
        if (!row) continue;
        const enable = toEnable.includes(id);
        if (row.kind === "mcp") {
          await (enable ? api.enableMcpDefinitionProjects([projectId], id) : api.disableMcpDefinitionProjects([projectId], id));
        } else {
          await (enable ? api.enableExtensionSkillProjects([projectId], id) : api.disableExtensionSkillProjects([projectId], id));
        }
      }
      const parts = [];
      if (toEnable.length) parts.push(`启用 ${toEnable.length} 个`);
      if (toDisable.length) parts.push(`停用 ${toDisable.length} 个`);
      showToast(`已${parts.join("，")}`);
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return <Modal title="配置扩展" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted">为该项目添加或移除扩展配置；用户级已生效的扩展保持不变。</p>
    {!ordered.length
      ? <p className="small muted">没有可配置的扩展。</p>
      : <div className="extension-project-options">
        {ordered.map(row => {
          const projectEnabled = enabledIds.includes(row.id);
          const userEffective = userEffectiveIds.includes(row.id);
          return <label className="extension-check extension-project-option" key={row.id}>
            <input type="checkbox" disabled={busy || userEffective} checked={userEffective || selected.includes(row.id)}
              onChange={e => setSelected(current => e.target.checked ? [...current, row.id] : current.filter(id => id !== row.id))} />
            <Icon name={row.kind === "mcp" ? "mcp" : "skill"} />
            <span className="extension-project-name">{row.name}</span>
            {projectEnabled ? <span className="small muted">已启用</span> : userEffective && <span className="small muted">用户级已生效</span>}
          </label>;
        })}
      </div>}
    {error && <div role="alert" className="extension-error">{error}</div>}
    <div className="row extension-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || !changed} onClick={() => void submit()}>{busy ? "保存中…" : "保存"}</button>
    </div>
  </Modal>;
}
