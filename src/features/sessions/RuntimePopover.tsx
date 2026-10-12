import { useEffect, useRef, useState } from "react";
import Icon from "../../components/Icon";
import type { Agent, AgentRuntimeOverrides } from "../../types";
import type { useAgentRuntime } from "../agents/useAgentRuntime";

export interface RuntimePopoverProps {
  agent: Agent;
  runtime: ReturnType<typeof useAgentRuntime>;
  value: AgentRuntimeOverrides;
  onChange: (value: AgentRuntimeOverrides) => void;
  disabled?: boolean;
}

export default function RuntimePopover({
  runtime,
  value,
  onChange,
  disabled = false,
}: RuntimePopoverProps) {
  const [open, setOpen] = useState(false);
  const [subpage, setSubpage] = useState<"main" | "model" | "effort">("main");
  const popoverRef = useRef<HTMLDivElement>(null);

  // Close popover on outside click or Escape
  useEffect(() => {
    if (!open) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        setOpen(false);
        setSubpage("main");
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        setSubpage("main");
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  // When agent switches or catalog loads, discover options if needed
  useEffect(() => {
    if (open && runtime.settings) {
      void runtime.refresh();
    }
  }, [open, runtime.settings?.agent, runtime.refresh]);

  const models = runtime.options?.models ?? [];
  const effectiveModelId = value.model;
  const matchingModels = models.filter((m) => m.id === effectiveModelId && (!value.provider || m.provider === value.provider));
  const selectedModelObj = matchingModels.length === 1 ? matchingModels[0] : undefined;
  const activeModelLabel = selectedModelObj
    ? (selectedModelObj.display_name ?? selectedModelObj.id)
    : value.model ?? "Agent 默认";

  const hasPerModelEfforts = models.some((m) => m.supported_efforts.length > 0);
  const availableEfforts = selectedModelObj && hasPerModelEfforts
    ? selectedModelObj.supported_efforts
    : (runtime.options?.effort_levels ?? []);

  const supportsEffort = availableEfforts.length > 0;

  const effectiveEffort = value.effort;
  const activeEffortLabel = effectiveEffort ?? "";

  return (
    <div className="new-session-runtime-control" ref={popoverRef}>
      {/* 触发按钮：位于发送按钮左侧 */}
      <button
        type="button"
        className={`new-session-runtime-pill-btn${open ? " open" : ""}`}
        disabled={disabled}
        onClick={() => {
          setOpen((v) => !v);
          setSubpage("main");
        }}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label="模型与推理设置"
        title="模型与推理设置"
      >
        <span className="runtime-pill-model truncate">{activeModelLabel}</span>
        {activeEffortLabel && (
          <span className="runtime-pill-effort">{activeEffortLabel}</span>
        )}
        <Icon name="chevronDown" className={`runtime-pill-chevron${open ? " rotated" : ""}`} />
      </button>

      {/* 浮动卡片弹出层 */}
      {open && (
        <div className="new-session-runtime-popover" role="dialog" aria-label="模型与推理设置">
          {subpage === "main" && (
            <div className="new-session-popover-main">
              <div
                className="new-session-popover-row"
                role="button"
                tabIndex={0}
                onClick={() => setSubpage("model")}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setSubpage("model");
                  }
                }}
              >
                <span className="popover-row-label">模型</span>
                <span className="popover-row-val">
                  <span className="truncate">{activeModelLabel}</span>
                  <Icon name="chevronRight" />
                </span>
              </div>

              <div
                className={`new-session-popover-row ${!supportsEffort ? "disabled" : ""}`}
                role="button"
                tabIndex={supportsEffort ? 0 : -1}
                aria-disabled={!supportsEffort}
                onClick={() => {
                  if (supportsEffort) setSubpage("effort");
                }}
                onKeyDown={(e) => {
                  if (supportsEffort && (e.key === "Enter" || e.key === " ")) {
                    e.preventDefault();
                    setSubpage("effort");
                  }
                }}
              >
                <span className="popover-row-label">推理等级</span>
                <span className="popover-row-val">
                  <span className="truncate">{effectiveEffort || (supportsEffort ? "Agent 默认" : "不支持")}</span>
                  {supportsEffort && <Icon name="chevronRight" />}
                </span>
              </div>
            </div>
          )}

          {subpage === "model" && (
            <div className="new-session-popover-subpage">
              <div className="popover-subpage-head">
                <button
                  type="button"
                  className="btn ghost icon-button small popover-back-btn"
                  onClick={() => setSubpage("main")}
                  title="返回"
                >
                  <Icon name="chevronRight" className="rotate-180" />
                </button>
                <span className="popover-subpage-title">选择模型</span>
                <button
                  type="button"
                  className="btn ghost icon-button small"
                  disabled={runtime.loading}
                  onClick={() => void runtime.refresh(true)}
                  title="刷新模型列表"
                  aria-label="刷新模型列表"
                >
                  <Icon name="refresh" className={runtime.loading ? "spinning" : ""} />
                </button>
              </div>

              <div className="popover-options-list">
                <button type="button" className="popover-option-item" onClick={() => { onChange({ model: null, provider: null, effort: null }); setSubpage("main"); }}>Agent 默认</button>
                {models.map((m) => {
                  const isSelected = selectedModelObj === m;
                  return (
                    <button
                      key={`${m.provider ?? ""}/${m.id}`}
                      type="button"
                      className={`popover-option-item ${isSelected ? "selected" : ""}`}
                      onClick={() => {
                        const nextEfforts = hasPerModelEfforts
                          ? m.supported_efforts
                          : (runtime.options?.effort_levels ?? []);
                        const nextEffort = (value.effort && nextEfforts.includes(value.effort))
                          ? value.effort
                          : null;
                        onChange({ ...value, model: m.id, provider: m.provider, effort: nextEffort });
                        setSubpage("main");
                      }}
                      title={m.display_name ?? m.id}
                    >
                      <span className="truncate">{m.display_name ?? m.id}</span>
                      {isSelected && <Icon name="check" />}
                    </button>
                  );
                })}

                {runtime.error && <p role="alert" className="muted small">{runtime.error}</p>}
                {models.length === 0 && (
                  <div className="muted small" style={{ padding: "8px 12px" }}>
                    暂未发现模型，可点击右上角刷新
                  </div>
                )}
              </div>
            </div>
          )}

          {subpage === "effort" && (
            <div className="new-session-popover-subpage">
              <div className="popover-subpage-head">
                <button
                  type="button"
                  className="btn ghost icon-button small popover-back-btn"
                  onClick={() => setSubpage("main")}
                  title="返回"
                >
                  <Icon name="chevronRight" className="rotate-180" />
                </button>
                <span className="popover-subpage-title">选择推理等级</span>
                <div style={{ width: 24 }} />
              </div>

              <div className="popover-options-list">
                <button type="button" className="popover-option-item" onClick={() => { onChange({ ...value, effort: null }); setSubpage("main"); }}>Agent 默认</button>
                {availableEfforts.map((lvl) => {
                  const isSelected = effectiveEffort === lvl;
                  return (
                    <button
                      key={lvl}
                      type="button"
                      className={`popover-option-item ${isSelected ? "selected" : ""}`}
                      onClick={() => {
                        onChange({ ...value, model: effectiveModelId, effort: lvl });
                        setSubpage("main");
                      }}
                    >
                      <span className="truncate">{lvl}</span>
                      {isSelected && <Icon name="check" />}
                    </button>
                  );
                })}

                {availableEfforts.length === 0 && (
                  <div className="muted small" style={{ padding: "8px 12px" }}>
                    当前模型暂无可选推理等级
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
