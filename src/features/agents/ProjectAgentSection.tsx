import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { DetailContent, DetailSection } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import Icon from "../../components/Icon";
import Notice from "../../components/Notice";
import type { AgentFile, AgentFileName, ProjectAgent } from "../../types";
import { MD_COMPONENTS } from "../sessions/SessionMessage";
import "./agents.css";

const documents = [
  { file: "role", filename: "ROLE.md", title: "角色与职责", empty: "尚未设定角色与职责。" },
  { file: "soul", filename: "SOUL.md", title: "性格与交流方式", empty: "尚未设定性格与交流方式。" },
  { file: "agents", filename: "AGENTS.md", title: "运行指南", empty: "运行指南为空。" },
] as const;

const readOnlyDocuments = {
  intro: { filename: "INTRO.md", title: "自我介绍", empty: "智能体还没有写自我介绍。" },
  user: { filename: "USER.md", title: "用户印象", empty: "智能体还没有记录对用户的印象。" },
};

export function AgentReadOnlyDocument({ file, document, onRefresh }: {
  file: keyof typeof readOnlyDocuments;
  document: AgentFile;
  onRefresh: () => void;
}) {
  const { filename, title, empty } = readOnlyDocuments[file];
  const intro = file === "intro";
  const refreshLabel = `刷新${title}`;
  return <DetailSection sectionId={`agent-${file}`} label={title}
    title={<><Icon name={intro ? "spark" : "file"} /><span>{title}</span></>}
    actions={<button type="button" className="btn ghost icon-button small"
      title={refreshLabel} aria-label={refreshLabel} onClick={onRefresh}><Icon name="refresh" /></button>}>
    {document.content.trim() ? (
      <DetailContent label={`${title}内容`} className={intro ? "md-body agent-intro-body" : undefined}>
        {intro ? <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{document.content}</ReactMarkdown>
          : <p className="detail-description">{document.content}</p>}
      </DetailContent>
    ) : <p className="detail-empty">{document.state === "unavailable" ? `无法读取 ${filename}。` : empty}</p>}
  </DetailSection>;
}

type ArchiveProps = {
  projectId: string;
  archived: boolean;
  onSaved: (agent: ProjectAgent, file?: AgentFileName) => void;
  onRefresh: () => void;
};

export default function ProjectAgentSection({ agent, active, ...props }: ArchiveProps & {
  agent: ProjectAgent;
  active: boolean;
}) {
  const [confirmReset, setConfirmReset] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [resetError, setResetError] = useState("");
  const missingFiles = [
    ...documents.filter(({ file }) => file !== "agents" && agent[file].state === "missing").map(({ filename }) => filename),
    ...(agent.claude_md_missing ? ["CLAUDE.md"] : []),
  ];
  const canReset = !props.archived && agent.agents.state !== "unavailable"
    && (!agent.guide_is_default || missingFiles.length > 0);

  useEffect(() => {
    if (!active) setConfirmReset(false);
  }, [active]);

  async function resetFiles() {
    if (resetting || !canReset) return;
    setResetting(true);
    setResetError("");
    try {
      props.onSaved(await api.prepareAgentFiles(props.projectId));
      setConfirmReset(false);
    } catch (error) {
      setResetError(`重置运行指南失败：${String(error)}`);
      props.onRefresh();
    } finally {
      setResetting(false);
    }
  }

  return <>
    {missingFiles.length > 0 && <Notice tone="warning">档案文件缺失。</Notice>}
    {documents.map(({ file, ...document }) => (
      <AgentDocument key={file} {...props} {...document} file={file} document={agent[file]}
        onReset={file === "agents" && canReset ? () => { setResetError(""); setConfirmReset(true); } : undefined} />
    ))}
    <AgentReadOnlyDocument file="user" document={agent.user} onRefresh={props.onRefresh} />
    {active && confirmReset && (
      <Modal title="重置运行指南？" onClose={() => { if (!resetting) setConfirmReset(false); }}>
        <ul>
          <li><code>AGENTS.md</code> 恢复为 NoEnding 默认模板，现有的自定义内容会被替换。</li>
          {missingFiles.map(filename => <li key={filename}>补建 <code>{filename}</code>。</li>)}
        </ul>
        {resetError && <Notice tone="error">{resetError}</Notice>}
        <DialogActions>
          <button type="button" className="btn" disabled={resetting} onClick={() => setConfirmReset(false)}>取消</button>
          <button type="button" className="btn danger" disabled={resetting || !canReset} onClick={() => void resetFiles()}>
            {resetting ? "重置中…" : "确认重置"}
          </button>
        </DialogActions>
      </Modal>
    )}
  </>;
}

function AgentDocument({ projectId, archived, onSaved, onRefresh, file, title, filename, empty, document, onReset }: ArchiveProps & {
  file: AgentFileName;
  title: string;
  filename: string;
  empty: string;
  document: AgentFile;
  onReset?: () => void;
}) {
  const { content, revision, state } = document;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<{ value: string; revision: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const editButton = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef(false);
  const conflicted = draft !== null && draft.revision !== revision;
  const canEdit = !archived && revision !== null;
  const reset = editing ? undefined : onReset;

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
      onSaved(await api.updateAgentProfile(projectId, expected, { file, content: draft.value }), file);
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
    <DetailSection sectionId={`agent-${file}`} label={title}
      title={<><Icon name="file" /><span>{title}</span></>}
      status={state !== "current" && <span className="badge warn">{status}</span>}
      actions={canEdit || reset ? ({ expand }) => <>
        {reset && <button type="button" className="btn ghost small" onClick={reset}>重置运行指南</button>}
        {canEdit && <button ref={editButton} type="button" className="btn ghost icon-button small"
          disabled={editing} title={`编辑 ${title}`} aria-label={`编辑${title}`}
          onClick={() => { expand(); setError(""); setEditing(true); }}><Icon name="edit" /></button>}
      </> : undefined}>
      {error && <Notice tone="error">{error}</Notice>}
      {editing && !archived ? (
        <div className="instructions-editor">
          {conflicted && <Notice tone="warning">
            {filename} 已在外部修改。
            <div className="agent-editor-actions">
              <button type="button" className="btn small ghost" disabled={saving} onClick={closeEditor}>放弃草稿</button>
              <button type="button" className="btn small primary" disabled={saving || revision === null}
                onClick={() => { if (revision !== null) void saveDraft(revision); }}>覆盖文件修改</button>
            </div>
          </Notice>}
          <textarea className="input textarea" rows={6} value={draft?.value ?? content}
            disabled={saving} aria-label={filename} autoFocus
            onChange={event => { if (revision !== null) setDraft({ value: event.target.value, revision: draft?.revision ?? revision }); }} />
          <div className="agent-editor-actions">
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
