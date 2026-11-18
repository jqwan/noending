import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import CustomSelect from "../../components/CustomSelect";
import DialogActions from "../../components/DialogActions";
import Icon, { type IconName } from "../../components/Icon";
import Notice from "../../components/Notice";
import { AGENT_LABELS, type Agent, type AgentStatusEntry, type CreateAssistantInput } from "../../types";
import { clearAssistantsCache } from "./assistantsCache";

const AVATAR_OPTIONS: Array<{ token: string; icon: IconName; label: string }> = [
  { token: "spark", icon: "spark", label: "灵动 (Spark)" },
  { token: "agent", icon: "agent", label: "智能 (Agent)" },
  { token: "chat", icon: "chat", label: "对话 (Chat)" },
  { token: "tasks", icon: "tasks", label: "执行 (Tasks)" },
  { token: "desktop", icon: "desktop", label: "办公 (Desktop)" },
];

export default function CreateAssistantModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (id: string, name: string) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [agent, setAgent] = useState<Agent>("codex");
  const [avatar, setAvatar] = useState("spark");
  const [instructions, setInstructions] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [agentStatus, setAgentStatus] = useState<Record<Agent, AgentStatusEntry> | null>(null);
  const [loadingAgentStatus, setLoadingAgentStatus] = useState(false);
  const [agentStatusError, setAgentStatusError] = useState("");

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
    void loadAgentStatus();
  }, [loadAgentStatus]);

  const agentOptions = useMemo(() => {
    if (!agentStatus) return [];
    const allAgents = Object.keys(AGENT_LABELS) as Agent[];
    return allAgents
      .filter((a) => Boolean(agentStatus[a]?.terminal_cli))
      .map((a) => ({
        value: a,
        label: AGENT_LABELS[a] ?? a,
      }));
  }, [agentStatus]);

  useEffect(() => {
    if (agentOptions.length > 0 && !agentOptions.some((opt) => opt.value === agent)) {
      setAgent(agentOptions[0].value);
    }
  }, [agentOptions, agent]);

  if (!open) return null;

  const handleSubmit = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("请输入智能体名称");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const input: CreateAssistantInput = {
        name: trimmedName,
        agent,
        avatar,
        description: description.trim(),
        instructions: instructions.trim(),
      };
      const detail = await api.createAssistantProfile(input);
      clearAssistantsCache();
      onCreated(detail.profile.id, detail.profile.name);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="新建智能体"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <div className="assistant-modal">
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
        {!loadingAgentStatus && !agentStatusError && agentOptions.length === 0 && (
          <div style={{ marginBottom: "var(--space-md)" }}>
            <Notice tone="warning">
              当前系统未检测到具备终端 CLI 能力的运行工具，请先安装配置支持的 Agent。
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
            placeholder="例如：架构师小助手、敏捷需求梳理员"
            autoFocus
            disabled={busy}
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">职责简介</label>
          <input
            type="text"
            className="input"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="例如：梳理需求规格，拆解 Task 事项并核对工程交付"
            disabled={busy}
          />
        </div>

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">运行工具 (Agent)</label>
          <CustomSelect
            value={agent}
            onChange={(val) => setAgent(val as Agent)}
            options={agentOptions}
            ariaLabel="选择默认运行工具"
          />
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

        <div className="form-group" style={{ marginBottom: "var(--space-md)" }}>
          <label className="label bold">初始角色指令 (Prompt)</label>
          <textarea
            className="input textarea"
            rows={4}
            value={instructions}
            onChange={(e) => setInstructions(e.target.value)}
            placeholder="编写该智能体的系统设定、工作准则与任务调度规范（可在详情页随时编辑）…"
            disabled={busy}
          />
        </div>

        <DialogActions>
          <button
            type="button"
            className="btn ghost"
            onClick={onClose}
            disabled={busy}
          >
            取消
          </button>
          <button
            type="submit"
            className="btn primary"
            disabled={busy || !name.trim() || agentOptions.length === 0}
          >
            {busy ? "创建中…" : "创建智能体"}
          </button>
        </DialogActions>
      </form>
      </div>
    </Modal>
  );
}
