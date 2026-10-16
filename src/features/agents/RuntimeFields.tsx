import type { AgentRuntimeOverrides, AgentRuntimeDiscovery } from "../../types";

export const NATIVE_RUNTIME: AgentRuntimeOverrides = { model: null, provider: null, effort: null };

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
