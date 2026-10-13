import { useId } from "react";
import CustomSelect from "../../components/CustomSelect";
import type { AgentRuntimeOverrides, AgentRuntimeDiscovery } from "../../types";

export const NATIVE_RUNTIME: AgentRuntimeOverrides = { model: null, provider: null, effort: null };

export default function RuntimeFields({ value, options, onChange, onDiscover, onBlur, disabled = false }: {
  value: AgentRuntimeOverrides;
  options: AgentRuntimeDiscovery;
  onChange: (value: AgentRuntimeOverrides) => void;
  onDiscover: () => void;
  onBlur?: () => void;
  disabled?: boolean;
}) {
  const id = useId();
  const models = options.models.filter((m) => !value.provider || m.provider === value.provider);
  const matching = models.filter((m) => m.id === value.model);
  const selected = matching.length === 1 ? matching[0] : undefined;
  const hasPerModelEfforts = options.models.some(model => model.supported_efforts.length > 0);
  const efforts = selected && hasPerModelEfforts ? selected.supported_efforts : options.effort_levels;
  const change = (field: "model" | "provider" | "effort", raw: string) => {
    const next = { ...value, [field]: raw.trim() || null };
    if (field === "provider" && next.provider !== value.provider) {
      next.model = null;
      next.effort = null;
    }
    if (field === "model") {
      const matches = models.filter((m) => m.id === next.model);
      const model = matches.length === 1 ? matches[0] : undefined;
      if (model && hasPerModelEfforts && next.effort && !model.supported_efforts.includes(next.effort)) next.effort = null;
    }
    onChange(next);
  };
  return (
    <div className="agent-runtime-fields">
      {options.capabilities.provider !== "unsupported" && (
        <label className="agent-runtime-field">
          <span className="agent-runtime-field-title">提供商</span>
          <input
            type="text"
            aria-label="提供商"
            list={`${id}-providers`}
            value={value.provider ?? ""}
            placeholder="Agent 默认"
            disabled={disabled}
            onFocus={onDiscover}
            onBlur={onBlur}
            onChange={(e) => change("provider", e.target.value)}
          />
          <datalist id={`${id}-providers`}>
            {[...new Set(options.models.map((m) => m.provider).filter(Boolean))].map((p) => (
              <option key={p} value={p!} />
            ))}
          </datalist>
        </label>
      )}
      {options.capabilities.model !== "unsupported" && (
        <label className="agent-runtime-field">
          <span className="agent-runtime-field-title">模型</span>
          <input
            type="text"
            aria-label="模型"
            list={`${id}-models`}
            value={value.model ?? ""}
            placeholder="Agent 默认；可手动输入"
            disabled={disabled}
            onFocus={onDiscover}
            onBlur={onBlur}
            onChange={(e) => change("model", e.target.value)}
          />
          <datalist id={`${id}-models`}>
            {models.map((m) => (
              <option key={`${m.provider}/${m.id}`} value={m.id}>
                {m.provider ? `${m.provider} · ` : ""}{m.display_name ?? m.id}
              </option>
            ))}
          </datalist>
        </label>
      )}
      {options.capabilities.effort !== "unsupported" && (
        <div className="agent-runtime-field">
          <span className="agent-runtime-field-title">推理强度</span>
          <CustomSelect
            ariaLabel="推理强度"
            value={value.effort ?? ""}
            disabled={disabled}
            onChange={(raw) => change("effort", raw)}
            options={[
              { value: "", label: "Agent 默认" },
              ...efforts.map((e) => ({ value: e, label: e })),
              ...(value.effort && !efforts.includes(value.effort)
                ? [{ value: value.effort, label: `${value.effort}（已保存）` }]
                : []),
            ]}
          />
        </div>
      )}
    </div>
  );
}

export function CatalogStatus({ options, saving = false }: { options: AgentRuntimeDiscovery; saving?: boolean }) {
  return (
    <div className="muted small agent-catalog-status">
      <div className="agent-catalog-source-text">
        {options.model_source === "dynamic"
          ? `来自 CLI · ${options.models.length} 个模型`
          : options.model_source === "suggested"
          ? "建议模型别名，并非账户完整模型列表"
          : options.model_source === "unavailable"
          ? "无法获取模型列表，可使用 Agent 默认或手动输入"
          : "点击模型输入框获取列表，也可直接手动输入"}
        {saving && <span style={{ marginLeft: 8, color: "var(--accent)" }}>· 保存中…</span>}
      </div>
      {options.warnings.map((w) => (
        <div key={w} className="agent-catalog-warning">{w}</div>
      ))}
    </div>
  );
}
