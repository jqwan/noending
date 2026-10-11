import { useId } from "react";
import type { AgentRuntimeOverrides, AgentRuntimeDiscovery } from "../../types";

export const NATIVE_RUNTIME: AgentRuntimeOverrides = { model: null, provider: null, effort: null };

export default function RuntimeFields({ value, options, onChange, onDiscover, disabled = false }: {
  value: AgentRuntimeOverrides;
  options: AgentRuntimeDiscovery;
  onChange: (value: AgentRuntimeOverrides) => void;
  onDiscover: () => void;
  disabled?: boolean;
}) {
  const id = useId();
  const models = options.models.filter((m) => !value.provider || m.provider === value.provider);
  const matching = models.filter((m) => m.id === value.model);
  const selected = matching.length === 1 ? matching[0] : undefined;
  const efforts = selected?.supported_efforts.length ? selected.supported_efforts : options.effort_levels;
  const change = (field: "model" | "provider" | "effort", raw: string) => {
    const next = { ...value, [field]: raw.trim() || null };
    if (field === "provider" && next.provider !== value.provider) {
      next.model = null;
      next.effort = null;
    }
    if (field === "model") {
      const matches = models.filter((m) => m.id === next.model);
      const model = matches.length === 1 ? matches[0] : undefined;
      if (model?.supported_efforts.length && next.effort && !model.supported_efforts.includes(next.effort)) next.effort = null;
    }
    onChange(next);
  };
  return (
    <div className="agent-runtime-fields">
      {options.capabilities.provider !== "unsupported" && <label>提供商
        <input type="text" aria-label="提供商" list={`${id}-providers`} value={value.provider ?? ""} placeholder="Agent 默认" disabled={disabled} onFocus={onDiscover} onChange={(e) => change("provider", e.target.value)} />
        <datalist id={`${id}-providers`}>{[...new Set(options.models.map((m) => m.provider).filter(Boolean))].map((p) => <option key={p} value={p!} />)}</datalist>
      </label>}
      {options.capabilities.model !== "unsupported" && <label>模型
        <input type="text" aria-label="模型" list={`${id}-models`} value={value.model ?? ""} placeholder="Agent 默认；可手动输入" disabled={disabled} onFocus={onDiscover} onChange={(e) => change("model", e.target.value)} />
        <datalist id={`${id}-models`}>{models.map((m) => <option key={`${m.provider}/${m.id}`} value={m.id}>{m.provider ? `${m.provider} · ` : ""}{m.display_name ?? m.id}</option>)}</datalist>
      </label>}
      {options.capabilities.effort !== "unsupported" && <label>推理强度
        <select aria-label="推理强度" value={value.effort ?? ""} disabled={disabled} onChange={(e) => change("effort", e.target.value)}>
          <option value="">Agent 默认</option>
          {efforts.map((e) => <option key={e} value={e}>{e}</option>)}
          {value.effort && !efforts.includes(value.effort) && <option value={value.effort}>{value.effort}（已保存）</option>}
        </select>
      </label>}
    </div>
  );
}

export function CatalogStatus({ options }: { options: AgentRuntimeDiscovery }) {
  return <div className="muted small agent-catalog-status">
    {options.model_source === "dynamic" ? `来自 CLI · ${options.models.length} 个模型` : options.model_source === "suggested" ? "建议模型别名，并非账户完整模型列表" : options.model_source === "unavailable" ? "无法获取模型列表，可使用 Agent 默认或手动输入" : "点击模型输入框获取列表，也可直接手动输入"}
    {options.warnings.map((w) => <div key={w}>{w}</div>)}
  </div>;
}
