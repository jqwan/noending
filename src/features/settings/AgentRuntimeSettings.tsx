import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type AgentRuntimeOverrides, type AgentRuntimeSettings,
  type ModelSource, type RuntimeFieldCapability, type RuntimeModelOption } from "../../types";

type Field = keyof AgentRuntimeOverrides;

const DEFAULT_VALUE = "__default__";
const CUSTOM_VALUE = "__custom__";

/** 同一种 override，在三家 CLI 里的叫法不同；unsupported 的字段不展示（§11）。 */
export const FIELD_LABELS: Record<Agent, Record<Field, string>> = {
  codex: { model: "Model", provider: "Provider", effort: "Reasoning" },
  claude_code: { model: "Model", provider: "Provider", effort: "Effort" },
  pi: { model: "Model", provider: "Provider", effort: "Thinking" },
  // Qoder 没有 CLI，三个字段都是 unsupported（后端 capabilities 也是这么给的），
  // 这里的字面量只是为了让 Record 完整——unsupported 的字段不会渲染（§37.5）。
  qoder: { model: "Model", provider: "Provider", effort: "Effort" },
  autoclaw: { model: "Model", provider: "Provider", effort: "Effort" },
  workbuddy: { model: "Model", provider: "Provider", effort: "Effort" },
  dsh: { model: "Model", provider: "Provider", effort: "Effort" },
  gemini: { model: "Model", provider: "Provider", effort: "Effort" },
  zcode: { model: "Model", provider: "Provider", effort: "Effort" },
};

const SOURCE_NOTE: Record<ModelSource, string | null> = {
  not_loaded: "尚未刷新模型列表；「Agent 默认值」与「自定义…」始终可用。",
  dynamic: null,
  suggested: "以下是 NoEnding 的建议值，不代表你账号当前可用的完整模型列表。",
  unavailable: "无法从 Agent 获取模型列表；「Agent 默认值」与「自定义」仍然可用。",
};

export function isOverridable(cap: RuntimeFieldCapability) {
  return cap !== "unsupported";
}

/**
 * Preview 里的 Runtime 意图：显示的就是 Launch 将要（或不会）传给 CLI 的参数。
 * 数据来源是 PreparedLaunch 中冻结的 override，不是重新读取的设置。
 */
export function RuntimeIntentBadges({ agent, runtime }: {
  agent: Agent;
  runtime: AgentRuntimeOverrides;
}) {
  const fields = Object.keys(FIELD_LABELS[agent]) as Field[];
  const overridden = fields.filter((f) => runtime[f] !== null);
  if (overridden.length === 0) {
    return <span className="badge">Runtime：Agent 默认值</span>;
  }
  return (
    <>
      {overridden.map((f) => (
        <span className="badge accent" key={f}>
          {FIELD_LABELS[agent][f]}: {runtime[f]}
        </span>
      ))}
      {overridden.length < fields.length && (
        <span className="badge">其余为 Agent 默认值</span>
      )}
    </>
  );
}

/**
 * 设置 → Agent 的一块：安装状态 + 该 Agent 的 Runtime Overrides。
 * 每个字段只有两种状态——Agent 默认值（不传参数）或显式 Override。
 */
export default function AgentRuntimeRow({ agent }: { agent: Agent }) {
  const [st, setSt] = useState<AgentRuntimeSettings | null>(null);
  const [models, setModels] = useState<RuntimeModelOption[]>([]);
  const [modelSource, setModelSource] = useState<ModelSource>("not_loaded");
  const [warnings, setWarnings] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [custom, setCustom] = useState<Field | null>(null);
  const [customText, setCustomText] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getAgentRuntimeSettings(agent).then(setSt).catch(console.error);
  }, [agent]);

  // Discovery 是建议信息：单独取，失败只留 warning，不影响 override 显示。
  // 进入页面 ≠ 刷新模型：mount 阶段绝不 spawn Agent CLI（方案 §1/§9），
  // 「刷新模型」按钮是唯一的 discovery 入口。
  const refresh = useCallback(async () => {
    setLoadingModels(true);
    try {
      const d = await api.refreshAgentRuntimeOptions(agent);
      setModels(d.models);
      setModelSource(d.model_source);
      setWarnings(d.warnings);
      setError(null);
    } catch (e) {
      // 失败只降级 catalog：已保存的 override 原样保留（方案 §11）。
      setModelSource("unavailable");
      setWarnings([String(e)]);
    } finally {
      setLoadingModels(false);
    }
  }, [agent]);

  if (!st) {
    return (
      <div className="settings-agent-runtime muted small">
        {AGENT_LABELS[agent]} 加载中…
      </div>
    );
  }

  const overrides = st.overrides;
  const caps = st.capabilities;
  const effortLevels = st.effort_levels;

  const providers = Array.from(new Set(models.map((m) => m.provider).filter(Boolean))) as string[];

  const optionsFor = (field: Field): { value: string; label: string }[] => {
    if (field === "provider") {
      return providers.map((p) => ({ value: p, label: p }));
    }
    if (field === "model") {
      // Provider 被显式覆盖时按 provider 过滤，但当前值始终保留可选。
      const picked = overrides.provider;
      return models
        .filter((m) => !picked || m.provider === picked)
        .map((m) => ({ value: m.id, label: m.display_name ?? m.id }));
    }
    const model = models.find((m) => m.id === overrides.model);
    const levels = model?.supported_efforts.length ? model.supported_efforts : effortLevels;
    return levels.map((l) => ({ value: l, label: l }));
  };

  const save = async (next: AgentRuntimeOverrides) => {
    try {
      const fresh = await api.setAgentRuntimeOverrides(agent, next);
      setSt(fresh);
      setError(null);
    } catch (e) {
      // 后端拒绝（例如该 Agent 不支持这个字段）：保留原值并说明原因。
      setError(String(e));
    }
  };

  const pick = (field: Field, value: string) => {
    if (value === CUSTOM_VALUE) {
      setCustom(field);
      setCustomText("");
      return;
    }
    setCustom(null);
    void save({ ...overrides, [field]: value === DEFAULT_VALUE ? null : value });
  };

  const commitCustom = (field: Field) => {
    const v = customText.trim();
    setCustom(null);
    void save({ ...overrides, [field]: v === "" ? null : v });
  };

  const renderField = (field: Field) => {
    if (!isOverridable(caps[field])) return null;
    const value = overrides[field];
    const known = optionsFor(field);
    const isCustomValue = value !== null && !known.some((o) => o.value === value);
    const selectValue = custom === field ? CUSTOM_VALUE : (value ?? DEFAULT_VALUE);
    const label = FIELD_LABELS[agent][field];

    return (
      <label className="field" key={field}>
        <span>
          {label}
          {value === null ? "" : <em className="runtime-override-tag">Override</em>}
        </span>
        <select value={selectValue} onChange={(e) => pick(field, e.target.value)}>
          <option value={DEFAULT_VALUE}>Agent 默认值</option>
          {known.map((o) => (
            <option key={o.value} value={o.value}>{o.label}</option>
          ))}
          {isCustomValue && <option value={value}>{value}</option>}
          <option value={CUSTOM_VALUE}>自定义…</option>
        </select>
        {custom === field ? (
          <input
            type="text"
            autoFocus
            value={customText}
            placeholder={field === "model" ? "模型 ID" : "自定义值"}
            onChange={(e) => setCustomText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229) commitCustom(field);
              if (e.key === "Escape") setCustom(null);
            }}
            onBlur={() => commitCustom(field)}
          />
        ) : (
          value !== null && (
            <span className="muted small">NoEnding 启动该 Agent 时会显式传 {label}。</span>
          )
        )}
      </label>
    );
  };

  return (
    <div className="settings-agent-runtime">
      <div className="row-line">
        <div>
          <div className="settings-row-label row" style={{ gap: 7 }}>
            <AgentIcon agent={agent} />
            {AGENT_LABELS[agent]}
          </div>
          <div className="settings-row-hint mono">{st.executable ?? "未找到可执行文件"}</div>
        </div>
        <div className="row">
          {st.version && <span className="muted mono small">{st.version}</span>}
          <span className={`badge ${st.detected ? "success" : ""}`}>
            {st.detected ? "已检测" : "未检测"}
          </span>
        </div>
      </div>

      <div className="runtime-fields">
        {(["provider", "model", "effort"] as Field[]).map(renderField)}
      </div>

      <div className="runtime-footer">
        <button className="btn ghost small" onClick={() => void refresh()} disabled={loadingModels}>
          {loadingModels ? "正在刷新…" : "刷新模型"}
        </button>
        <span className="muted small">
          {SOURCE_NOTE[modelSource] ?? `${models.length} 个模型来自 ${AGENT_LABELS[agent]} CLI`}
        </span>
      </div>

      {warnings.map((w) => (
        <p className="muted small runtime-note" key={w}>{w}</p>
      ))}
      {error && <p className="small runtime-note" style={{ color: "var(--danger)" }}>{error}</p>}
    </div>
  );
}
