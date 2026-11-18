import { useCallback, useEffect, useMemo, useState } from "react";
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
  const [name, setName] = useState(profile.name);
  const [description, setDescription] = useState(profile.description);
  const [agent, setAgent] = useState<Agent>(profile.agent);
  const [avatar, setAvatar] = useState(profile.avatar);
  const [runtimeOverride, setRuntimeOverride] = useState<AgentRuntimeOverrides>(
    profile.runtime_override ?? NATIVE_RUNTIME,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [agentStatus, setAgentStatus] = useState<Record<Agent, AgentStatusEntry> | null>(null);
  const [loadingAgentStatus, setLoadingAgentStatus] = useState(false);
  const [agentStatusError, setAgentStatusError] = useState("");

  const runtimeHook = useAgentRuntime(agent, open);

  useEffect(() => {
    setName(profile.name);
    setDescription(profile.description);
    setAgent(profile.agent);
    setAvatar(profile.avatar);
    setRuntimeOverride(profile.runtime_override ?? NATIVE_RUNTIME);
    setError("");
  }, [profile, open]);

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
      const isDefaultRuntime =
        !runtimeOverride.model && !runtimeOverride.provider && !runtimeOverride.effort;
      const patch: UpdateAssistantPatch = {
        name: trimmedName,
        avatar,
        description: description.trim(),
        agent,
        runtime_override: isDefaultRuntime ? null : runtimeOverride,
      };
      const updated = await api.updateAssistantProfile(profile.id, profile.revision, patch);
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
          <label className="label bold">
            名称 <span style={{ color: "var(--danger)" }}>*</span>
          </label>
          <input
            type="text"
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            disabled={busy}
            autoFocus
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">职责简介</label>
          <input
            type="text"
            className="input"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={busy}
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">运行工具 (Agent)</label>
          <CustomSelect
            value={agent}
            onChange={(val) => {
              const nextAgent = val as Agent;
              setAgent(nextAgent);
              setRuntimeOverride(NATIVE_RUNTIME);
            }}
            options={agentOptions}
            disabled={busy || loadingAgentStatus}
            ariaLabel="选择运行工具"
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">模型与推理配置</label>
          <div style={{ display: "flex", alignItems: "center", gap: "var(--space-sm)" }}>
            <RuntimePopover
              agent={agent}
              runtime={runtimeHook}
              value={runtimeOverride}
              onChange={setRuntimeOverride}
              disabled={busy}
              direction="down"
            />
            {(runtimeOverride.model || runtimeOverride.effort) && (
              <button
                type="button"
                className="btn ghost small"
                onClick={() => setRuntimeOverride(NATIVE_RUNTIME)}
                disabled={busy}
                title="重置为 Agent 默认模型与推理等级"
              >
                重置默认
              </button>
            )}
          </div>
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
            className="btn solid"
            disabled={busy}
          >
            {busy ? "保存中…" : "保存"}
          </button>
        </DialogActions>
      </form>
    </Modal>
  );
}
