import { useId, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import DialogActions from "../../components/DialogActions";
import Notice from "../../components/Notice";
import type { CreateAssistantInput } from "../../types";

export default function CreateAssistantModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (id: string, name: string) => void;
}) {
  const formId = useId();
  const [name, setName] = useState("");
  const [soul, setSoul] = useState("");
  const [role, setRole] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

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
        soul_md: soul.trim() ? soul : undefined,
        role_md: role.trim() ? role : undefined,
      };
      const project = await api.createAssistantProfile(input);
      onCreated(project.id, project.name);
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
            placeholder="例如：架构师小助手、敏捷需求梳理员"
            autoFocus
            disabled={busy}
          />
        </div>

        {[
          { id: "soul", label: "SOUL.md", value: soul, setValue: setSoul, placeholder: "描述价值观、行为原则和交流风格（可选）" },
          { id: "role", label: "ROLE.md", value: role, setValue: setRole, placeholder: "描述长期目标、职责和工作范围（可选）" },
        ].map(({ id, label, value, setValue, placeholder }) => (
          <div key={id} className="form-group" style={{ marginBottom: "var(--space-md)" }}>
            <label className="label bold" htmlFor={`${formId}-${id}`}>{label}</label>
            <textarea
              id={`${formId}-${id}`}
              className="input textarea"
              rows={3}
              value={value}
              onChange={e => setValue(e.target.value)}
              placeholder={placeholder}
              disabled={busy}
            />
          </div>
        ))}

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
            disabled={busy || !name.trim()}
          >
            {busy ? "创建中…" : "创建智能体"}
          </button>
        </DialogActions>
      </form>
      </div>
    </Modal>
  );
}
