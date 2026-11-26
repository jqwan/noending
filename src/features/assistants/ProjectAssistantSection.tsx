import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { DetailContent, DetailSection } from "../../components/DetailSection";
import Icon from "../../components/Icon";
import Notice from "../../components/Notice";
import type { AssistantFile, AssistantFileName, ProjectAssistant } from "../../types";
import "./assistants.css";

const documents = [
  { file: "role", filename: "ROLE.md", title: "长期职责", empty: "尚未设定长期职责。" },
  { file: "soul", filename: "SOUL.md", title: "行为与交流", empty: "尚未设定行为与交流方式。" },
] as const;

const guideIssues = {
  missing: "通用运行指南缺失。",
  outdated: "通用运行指南需要修复。",
  unavailable: "无法读取通用运行指南，请检查档案目录中的 AGENTS.md。",
};

type ArchiveProps = {
  projectId: string;
  archived: boolean;
  onSaved: (assistant: ProjectAssistant, file?: AssistantFileName) => void;
  onRefresh: () => void;
};

export default function ProjectAssistantSection({ assistant, ...props }: ArchiveProps & {
  assistant: ProjectAssistant;
}) {
  const [preparing, setPreparing] = useState(false);
  const [prepareError, setPrepareError] = useState("");

  async function prepareFiles() {
    setPreparing(true);
    setPrepareError("");
    try {
      props.onSaved(await api.prepareAssistantFiles(props.projectId));
    } catch (error) {
      setPrepareError(`修复档案文件失败：${String(error)}`);
    } finally {
      setPreparing(false);
    }
  }

  const guideState = assistant.guide_state;

  return <>
    {guideState !== "current" && (
      <Notice tone="warning">
        {guideIssues[guideState]}
        {!props.archived && guideState !== "unavailable" && (
          <div className="assistant-editor-actions">
            <button type="button" className="btn small" disabled={preparing}
              onClick={() => void prepareFiles()}>修复运行指南</button>
          </div>
        )}
      </Notice>
    )}
    {prepareError && <Notice tone="error">{prepareError}</Notice>}
    {documents.map(({ file, ...document }) => (
      <AssistantDocument key={file} {...props} {...document} file={file} document={assistant[file]}
        preparing={preparing} onPrepare={prepareFiles} />
    ))}
  </>;
}

function AssistantDocument({ projectId, archived, onSaved, onRefresh, file, title, filename, empty, document, preparing, onPrepare }: ArchiveProps & {
  file: AssistantFileName;
  title: string;
  filename: string;
  empty: string;
  document: AssistantFile;
  preparing: boolean;
  onPrepare: () => Promise<void>;
}) {
  const { content, revision, state } = document;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<{ value: string; revision: string } | null>(null);
  const [saving, setSaving] = useState(false);
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

  async function saveDraft(expected: string) {
    if (!draft || saving) return;
    setSaving(true);
    setError("");
    try {
      onSaved(await api.updateAssistantProfile(projectId, expected, { file, content: draft.value }), file);
      closeEditor();
    } catch (error) {
      setError(`保存 ${filename} 失败：${String(error)}`);
      onRefresh();
    } finally {
      setSaving(false);
    }
  }

  const status = state === "missing" ? "文件缺失" : "无法读取";

  return (
    <DetailSection sectionId={`assistant-${file}`} label={title}
      title={<><Icon name="file" /><span>{title}</span></>}
      status={state !== "current" && <span className="badge warn">{status}</span>}
      actions={({ expand }) => !archived && <>
        {state === "missing" && <button type="button" className="btn ghost icon-button small"
          disabled={preparing} onClick={() => { expand(); void onPrepare(); }}
          title="补建档案文件" aria-label={`补建 ${filename}`}><Icon name="plus" /></button>}
        {revision !== null && <button ref={editButton} type="button" className="btn ghost icon-button small"
          disabled={editing} title={`编辑 ${title}`} aria-label={`编辑 ${filename}`}
          onClick={() => { expand(); setError(""); setEditing(true); }}><Icon name="edit" /></button>}
      </>}>
      {error && <Notice tone="error">{error}</Notice>}
      {editing && !archived ? (
        <div className="instructions-editor">
          {conflicted && <Notice tone="warning">
            {filename} 已在外部修改。
            <div className="assistant-editor-actions">
              <button type="button" className="btn small ghost" disabled={saving} onClick={closeEditor}>放弃草稿</button>
              <button type="button" className="btn small primary" disabled={saving || revision === null}
                onClick={() => { if (revision !== null) void saveDraft(revision); }}>覆盖文件修改</button>
            </div>
          </Notice>}
          <textarea className="input textarea" rows={6} value={draft?.value ?? content}
            disabled={saving} aria-label={filename} autoFocus
            onChange={event => { if (revision !== null) setDraft({ value: event.target.value, revision: draft?.revision ?? revision }); }} />
          <div className="assistant-editor-actions">
            <button type="button" className="btn small" disabled={saving} onClick={closeEditor}>取消</button>
            <button type="button" className="btn small primary" disabled={!draft || saving}
              onClick={() => { if (draft) void saveDraft(draft.revision); }}>{saving ? "保存中…" : "保存修改"}</button>
          </div>
        </div>
      ) : content.trim() ? (
        <DetailContent label={`${title} 内容`}><p className="detail-description">{content}</p></DetailContent>
      ) : <p className="detail-empty">{state === "current" ? empty : status}</p>}
    </DetailSection>
  );
}
