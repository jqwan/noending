import { useId, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import DialogActions from "../../components/DialogActions";
import Notice from "../../components/Notice";
import type { CreateAgentInput } from "../../types";

export default function CreateAgentModal({
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
      const input: CreateAgentInput = {
        name: trimmedName,
        soul_md: soul.trim() ? soul : undefined,
        role_md: role.trim() ? role : undefined,
      };
      const project = await api.createAgentProfile(input);
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
      <div className="agent-modal">
        <form onSubmit={handleSubmit} className="agent-form">
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
            placeholder="例如：项目经理、架构顾问"
            autoFocus
            disabled={busy}
          />
        </div>

        {[
          { id: "soul", label: "性格与交流方式", value: soul, setValue: setSoul, placeholder: "例如，我先说结论，再说明依据。（可选）" },
          { id: "role", label: "角色与职责", value: role, setValue: setRole, placeholder: "例如，我是发布协调员，负责协调分工和核对交付。（可选）" },
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
