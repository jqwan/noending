import { useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { SkillResource } from "../../types";
import "./skills.css";

/** Additive project-level configuration for registered Skills. */
export default function ProjectSkillDialog({ projectId, resources, enabledIds, userEffectiveIds, physicalIds, onClose, onDone }: {
  projectId: string; resources: SkillResource[]; enabledIds: string[]; userEffectiveIds: string[];
  physicalIds: string[];
  onClose: () => void; onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ordered = [...resources].sort((a, b) => a.name.localeCompare(b.name, "zh-Hans"));
  const removableIds = enabledIds.filter(id => !userEffectiveIds.includes(id) && !physicalIds.includes(id));
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
        await (enable ? api.enableSkillProjects([projectId], id) : api.disableSkillProjects([projectId], id));
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

  return <Modal title="配置技能" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted">为该项目添加或移除技能配置；用户级已生效的 Skill 保持不变。</p>
    {physicalIds.length > 0 && <p className="small muted">物理入口会保持启用，迁移源目录后才能直接停用。</p>}
    {!ordered.length
      ? <p className="small muted">没有可配置的技能。</p>
      : <div className="skill-project-options">
        {ordered.map(row => {
          const projectEnabled = enabledIds.includes(row.id);
          const userEffective = userEffectiveIds.includes(row.id);
          const physical = physicalIds.includes(row.id);
          return <label className="skill-check skill-project-option" key={row.id}>
            <input type="checkbox" disabled={busy || userEffective || physical} checked={userEffective || physical || selected.includes(row.id)}
              onChange={e => setSelected(current => e.target.checked ? [...current, row.id] : current.filter(id => id !== row.id))} />
            <Icon name="skill" />
            <span className="skill-project-name">{row.name}</span>
            {physical ? <span className="small muted">物理入口</span> : projectEnabled ? <span className="small muted">已启用</span> : userEffective && <span className="small muted">用户级已生效</span>}
          </label>;
        })}
      </div>}
    {error && <div role="alert" className="skill-error">{error}</div>}
    <div className="row skill-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || !changed} onClick={() => void submit()}>{busy ? "保存中…" : "保存"}</button>
    </div>
  </Modal>;
}
