import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { DetailContent, DetailSection } from "../../components/DetailSection";
import Icon from "../../components/Icon";
import Notice from "../../components/Notice";
import type { ProjectAssistant } from "../../types";
import "./assistants.css";

export default function ProjectAssistantSection({ projectId, assistant, archived, onSaved, onRefresh }: {
  projectId: string;
  assistant: ProjectAssistant;
  archived: boolean;
  onSaved: (assistant: ProjectAssistant) => void;
  onRefresh: () => void;
}) {
  const { agents_md: content, revision, agents_md_state: fileState } = assistant;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<{ value: string; revision: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState("");
  const editButton = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef(false);
  const conflicted = draft !== null && draft.revision !== revision;

  useEffect(() => {
    if (!editing && returnFocus.current) {
      returnFocus.current = false;
      editButton.current?.focus();
    }
  }, [editing]);

  function closeEditor() {
    returnFocus.current = true;
    setDraft(null);
    setError("");
    setEditing(false);
  }

  const save = async (revision: string) => {
    if (!draft || saving) return;
    setSaving(true);
    setError("");
    try {
      onSaved(await api.updateAssistantProfile(projectId, revision, { agents_md: draft.value }));
      closeEditor();
    } catch (error) {
      setError(`保存 AGENTS.md 失败：${String(error)}`);
      onRefresh();
    } finally {
      setSaving(false);
    }
  };

  const createFile = async () => {
    setSyncing(true);
    setError("");
    try {
      onSaved(await api.prepareAssistantFiles(projectId));
    } catch (error) {
      setError(`创建 AGENTS.md 失败：${String(error)}`);
    } finally {
      setSyncing(false);
    }
  };

  const status = fileState === "missing" ? "文件缺失" : "无法读取";

  return (
    <DetailSection sectionId="assistant-instructions" label="AGENTS.md"
      title={<><Icon name="file" /><span>AGENTS.md</span></>}
      status={fileState !== "current" && <span className="badge warn">{status}</span>}
      actions={({ expand }) => !archived && <>
        {fileState === "missing" && <button type="button" className="btn ghost icon-button small"
          disabled={syncing} onClick={() => { expand(); void createFile(); }}
          title="创建 AGENTS.md" aria-label="创建 AGENTS.md"><Icon name="plus" /></button>}
        <button ref={editButton} type="button" className="btn ghost icon-button small"
          disabled={editing || revision === null} title="编辑 AGENTS.md" aria-label="编辑 AGENTS.md"
          onClick={() => { expand(); setError(""); setEditing(true); }}><Icon name="edit" /></button>
      </>}>
      {error && <Notice tone="error">{error}</Notice>}
      {editing && !archived ? (
        <div className="instructions-editor">
          {conflicted && <Notice tone="warning">
            AGENTS.md 已在外部修改。
            <div className="assistant-editor-actions">
              <button type="button" className="btn small ghost" disabled={saving} onClick={closeEditor}>放弃草稿</button>
              <button type="button" className="btn small primary" disabled={saving || revision === null}
                onClick={() => { if (revision !== null) void save(revision); }}>覆盖文件修改</button>
            </div>
          </Notice>}
          <textarea className="input textarea" rows={6} value={draft?.value ?? content}
            disabled={saving} aria-label="AGENTS.md" autoFocus
            onChange={event => { if (revision !== null) setDraft({ value: event.target.value, revision: draft?.revision ?? revision }); }} />
          <div className="assistant-editor-actions">
            <button type="button" className="btn small" disabled={saving} onClick={closeEditor}>取消</button>
            <button type="button" className="btn small primary" disabled={!draft || saving}
              onClick={() => { if (draft) void save(draft.revision); }}>{saving ? "保存中…" : "保存修改"}</button>
          </div>
        </div>
      ) : content ? (
        <DetailContent label="AGENTS.md 内容"><p className="detail-description">{content}</p></DetailContent>
      ) : <p className="detail-empty">{fileState === "current" ? "文件为空。" : status}</p>}
    </DetailSection>
  );
}
