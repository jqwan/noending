import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import CustomSelect from "../../components/CustomSelect";
import DialogActions from "../../components/DialogActions";
import Icon, { type IconName } from "../../components/Icon";
import Notice from "../../components/Notice";
import { AGENT_LABELS, type Agent, type AgentRuntimeOverrides, type AgentStatusEntry, type AssistantDetail, type AssistantProfile, type UpdateAssistantPatch } from "../../types";
import { NATIVE_RUNTIME } from "../agents/RuntimeFields";
import { useAgentRuntime } from "../agents/useAgentRuntime";
import RuntimePopover from "../sessions/RuntimePopover";

const AVATAR_OPTIONS: Array<{ token: string; icon: IconName; label: string }> = [
  { token: "spark", icon: "spark", label: "灵动 (Spark)" },
  { token: "agent", icon: "agent", label: "智能 (Agent)" },
  { token: "chat", icon: "chat", label: "对话 (Chat)" },
  { token: "tasks", icon: "tasks", label: "执行 (Tasks)" },
  { token: "desktop", icon: "desktop", label: "办公 (Desktop)" },
];

export default function EditProfileModal({
  open,
  profile,
  onClose,
  onSaved,
}: {
  open: boolean;
  profile: AssistantProfile;
  onClose: () => void;
  onSaved: (detail: AssistantDetail) => void;
}) {
  const formId = useId();
  const [name, setName] = useState(profile.name);
  const [description, setDescription] = useState(profile.description);
  const [agent, setAgent] = useState<Agent>(profile.agent);
  const [avatar, setAvatar] = useState(profile.avatar);
  const [runtimeMode, setRuntimeMode] = useState<"inherit" | "native" | "custom">(() => {
    if (profile.runtime_override === null) return "inherit";
    if (!profile.runtime_override.model && !profile.runtime_override.provider && !profile.runtime_override.effort) return "native";
    return "custom";
  });
  const [runtimeOverride, setRuntimeOverride] = useState<AgentRuntimeOverrides>(
    profile.runtime_override ?? NATIVE_RUNTIME,
  );
  const [runtimeModified, setRuntimeModified] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [agentStatus, setAgentStatus] = useState<Record<Agent, AgentStatusEntry> | null>(null);
  const [loadingAgentStatus, setLoadingAgentStatus] = useState(false);
  const [agentStatusError, setAgentStatusError] = useState("");

  const baseRevisionRef = useRef(profile.revision);
  const wasOpenRef = useRef(false);
  const runtimeHook = useAgentRuntime(agent, open);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      baseRevisionRef.current = profile.revision;
      setName(profile.name);
      setDescription(profile.description);
      setAgent(profile.agent);
      setAvatar(profile.avatar);
      const mode = profile.runtime_override === null
        ? "inherit"
        : (!profile.runtime_override.model && !profile.runtime_override.provider && !profile.runtime_override.effort)
        ? "native"
        : "custom";
      setRuntimeMode(mode);
      setRuntimeOverride(profile.runtime_override ?? NATIVE_RUNTIME);
      setRuntimeModified(false);
      setError("");
    }
    wasOpenRef.current = open;
  }, [open, profile]);

  const loadAgentStatus = useCallback(async () => {
    setLoadingAgentStatus(true);
    setAgentStatusError("");
    try {
      const status = await api.getAgentStatus();
      setAgentStatus(status);
    } catch (err) {
      setAgentStatusError(`读取运行工具状态失败：${String(err)}`);
    } finally {
      setLoadingAgentStatus(false);
    }
  }, []);

  useEffect(() => {
    if (open) {
      void loadAgentStatus();
    }
  }, [open, loadAgentStatus]);

  const agentOptions = useMemo(() => {
    if (!agentStatus) return [];
    const allAgents = Object.keys(AGENT_LABELS) as Agent[];
    const cliAgents = allAgents.filter((a) => Boolean(agentStatus[a]?.terminal_cli));
    // 确保当前档案已配置的 agent 哪怕未探到也能展示当前值
    if (!cliAgents.includes(profile.agent)) {
      cliAgents.push(profile.agent);
    }
    return cliAgents.map((a) => ({
      value: a,
      label: AGENT_LABELS[a] ?? a,
    }));
  }, [agentStatus, profile.agent]);

  if (!open) return null;

  const handleSubmit = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("智能体名称不能为空");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const patch: UpdateAssistantPatch = {
        name: trimmedName,
        avatar,
        description: description.trim(),
        agent,
      };
      if (runtimeModified) {
        if (runtimeMode === "inherit") {
          patch.runtime_override = null;
        } else if (runtimeMode === "native") {
          patch.runtime_override = NATIVE_RUNTIME;
        } else {
          patch.runtime_override = runtimeOverride;
        }
      }
      const updated = await api.updateAssistantProfile(profile.id, baseRevisionRef.current, patch);
      onSaved(updated);
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="编辑智能体档案"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <form onSubmit={handleSubmit} className="assistant-form">
        {error && <Notice tone="error">{error}</Notice>}
        {agentStatusError && (
          <div style={{ marginBottom: "var(--space-md)" }}>
            <Notice tone="error">
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <span>{agentStatusError}</span>
                <button type="button" className="btn small ghost" onClick={() => void loadAgentStatus()}>
                  重试
                </button>
              </div>
            </Notice>
          </div>
        )}

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold" htmlFor={`${formId}-name`}>
            名称 <span style={{ color: "var(--danger)" }}>*</span>
          </label>
          <input
            id={`${formId}-name`}
            type="text"
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            disabled={busy}
            autoFocus
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold" htmlFor={`${formId}-description`}>职责简介</label>
          <input
            id={`${formId}-description`}
            type="text"
            className="input"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={busy}
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">运行工具</label>
          <CustomSelect
            value={agent}
            onChange={(val) => {
              const nextAgent = val as Agent;
              setAgent(nextAgent);
              setRuntimeMode("inherit");
              setRuntimeOverride(NATIVE_RUNTIME);
              setRuntimeModified(true);
            }}
            options={agentOptions}
            disabled={busy || loadingAgentStatus}
            ariaLabel="选择运行工具"
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">运行与推理配置</label>
          <div style={{ display: "flex", gap: "var(--space-md)", flexWrap: "wrap", marginBottom: "var(--space-xs)" }}>
            <label className="checkbox-row" style={{ display: "flex", alignItems: "center", gap: "var(--space-xs)", cursor: "pointer" }}>
              <input
                type="radio"
                name="runtimeMode"
                value="inherit"
                checked={runtimeMode === "inherit"}
                onChange={() => {
                  setRuntimeMode("inherit");
                  setRuntimeModified(true);
                }}
                disabled={busy}
              />
              <span className="small">继承应用默认</span>
            </label>
            <label className="checkbox-row" style={{ display: "flex", alignItems: "center", gap: "var(--space-xs)", cursor: "pointer" }}>
              <input
                type="radio"
                name="runtimeMode"
                value="native"
                checked={runtimeMode === "native"}
                onChange={() => {
                  setRuntimeMode("native");
                  setRuntimeModified(true);
                }}
                disabled={busy}
              />
              <span className="small">Agent CLI 默认</span>
            </label>
            <label className="checkbox-row" style={{ display: "flex", alignItems: "center", gap: "var(--space-xs)", cursor: "pointer" }}>
              <input
                type="radio"
                name="runtimeMode"
                value="custom"
                checked={runtimeMode === "custom"}
                onChange={() => {
                  setRuntimeMode("custom");
                  setRuntimeModified(true);
                }}
                disabled={busy}
              />
              <span className="small">自定义模型与推理</span>
            </label>
          </div>

          {runtimeMode === "custom" && (
            <div style={{ display: "flex", alignItems: "center", gap: "var(--space-sm)", marginTop: "var(--space-xs)" }}>
              <RuntimePopover
                agent={agent}
                runtime={runtimeHook}
                value={runtimeOverride}
                onChange={(val) => {
                  setRuntimeOverride(val);
                  setRuntimeModified(true);
                }}
                disabled={busy}
                direction="down"
              />
            </div>
          )}
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">头像图标</label>
          <div className="avatar-picker" role="radiogroup" aria-label="头像选择">
            {AVATAR_OPTIONS.map((item) => (
              <button
                key={item.token}
                type="button"
                className={`avatar-choice${avatar === item.token ? " selected" : ""}`}
                onClick={() => setAvatar(item.token)}
                aria-checked={avatar === item.token}
                role="radio"
                title={item.label}
              >
                <Icon name={item.icon} />
                <span className="small">{item.label.split(" ")[0]}</span>
              </button>
            ))}
          </div>
        </div>

        <DialogActions>
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={onClose}
          >
            取消
          </button>
          <button
            type="submit"
            className="btn primary"
            disabled={busy}
          >
            {busy ? "保存中…" : "保存"}
          </button>
        </DialogActions>
      </form>
    </Modal>
  );
}
