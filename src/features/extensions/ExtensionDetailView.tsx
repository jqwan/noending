import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import PageHeader from "../../layout/PageHeader";
import { AGENT_LABELS, type Agent, type ExtensionKind, type ExtensionLocation, type ExtensionProjectSkillScope, type ExtensionResourceDetail, type ExtensionSkillScopeEntry, type ExtensionSkillSourceKind, type ExtensionUninstallPreview } from "../../types";
import { EXTENSION_LABELS, locationSupportsAgent } from "./extensionPresentation";
import { EXTENSION_AGENTS } from "./extensionAgents";
import ExtensionConfigDialog from "./ExtensionConfigDialog";
import McpRegisterDialog from "./McpRegisterDialog";
import SkillRegisterDialog from "./SkillRegisterDialog";
import "./extensions.css";

function AgentIcons({ agents }: { agents: Agent[] }) {
  return <>{agents.map(agent => <span key={agent} role="img" aria-label={AGENT_LABELS[agent]} title={AGENT_LABELS[agent]}><AgentIcon agent={agent} size={16} /></span>)}</>;
}

const SOURCE_KIND_LABELS: Record<ExtensionSkillSourceKind, string> = {
  library: "NoEnding 源库",
  agent: "Agent 目录",
  external: "外部目录",
};

function scopeAgents(entries: ExtensionSkillScopeEntry[]) {
  const present = new Set(entries.flatMap(entry => entry.agents));
  return EXTENSION_AGENTS.filter(agent => present.has(agent));
}

export default function ExtensionDetailView({ resourceId, kind, initialTitle, navigate }: {
  resourceId: string; kind: ExtensionKind; initialTitle?: string;
  navigate: (route: Route) => void;
}) {
  const [locations, setLocations] = useState<ExtensionLocation[]>([]);
  const [detail, setDetail] = useState<ExtensionResourceDetail | null>(null);
  const [projectScopes, setProjectScopes] = useState<ExtensionProjectSkillScope[] | null>(null);
  const [sourceKind, setSourceKind] = useState<ExtensionSkillSourceKind | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyAction, setBusyAction] = useState<"" | "uninstall">("");
  const busy = busyAction !== "";
  const [error, setError] = useState("");
  const [removing, setRemoving] = useState<"skill" | "mcp" | null>(null);
  const [deleteSource, setDeleteSource] = useState(false);
  const [preview, setPreview] = useState<ExtensionUninstallPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [configuring, setConfiguring] = useState(false);
  const [registering, setRegistering] = useState(false);
  const previewRequest = useRef(0);
  const request = useRef(0);
  const alive = useRef(true);
  const pending = useRef(false);
  const board: Route = { view: "extensions", tab: kind };

  const refresh = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent) setLoading(true);
    try {
      const bundle = await api.getExtensionDetail(resourceId);
      if (seq !== request.current || !alive.current) return bundle !== null;
      setLocations(bundle?.locations ?? []);
      setDetail(bundle?.detail ?? null);
      setProjectScopes(bundle?.scopes ?? null);
      setSourceKind(bundle?.source_kind ?? null);
      setError(bundle ? "" : "此资源已移除或来源已变化，请返回扩展看板。");
      return bundle !== null;
    } catch (e) {
      if (seq === request.current && alive.current) { setDetail(null); setError(String(e)); }
      throw e;
    } finally { if (seq === request.current && alive.current && !silent) setLoading(false); }
  }, [resourceId]);
  useEffect(() => {
    alive.current = true;
    void refresh().catch(() => {});
    return () => { alive.current = false; ++request.current; };
  }, [refresh]);

  const operation = async (work: () => Promise<{ message: string }>) => {
    if (pending.current) return;
    pending.current = true; setBusyAction("uninstall"); setError("");
    try {
      const result = await work();
      if (!alive.current) return;
      showToast(result.message); setRemoving(null);
      if (!await refresh() && alive.current) navigate(board);
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusyAction(""); }
  };
  const row = detail?.resource;
  const deleteRequired = preview?.delete_source_required ?? row?.entry_links.some(link => !link) ?? false;
  const openSkillUninstall = async () => {
    if (!row) return;
    setError(""); setPreview(null); setDeleteSource(row.entry_links.some(link => !link)); setRemoving("skill");
    const seq = ++previewRequest.current;
    setPreviewing(true);
    try {
      const value = await api.extensionSkillUninstallPreview(row.id);
      if (!alive.current || seq !== previewRequest.current) return;
      setPreview(value); setDeleteSource(value.delete_source_required);
    } catch (e) {
      if (alive.current && seq === previewRequest.current) setError(String(e));
    } finally {
      if (alive.current && seq === previewRequest.current) setPreviewing(false);
    }
  };
  const closeRemove = () => {
    if (busy) return;
    ++previewRequest.current; setPreviewing(false); setRemoving(null); setPreview(null);
  };
  const openSourcePath = () => {
    if (!row?.path) return;
    void api.openPath(row.path).catch(e => setError(String(e)));
  };
  const userEntries = row ? row.entry_paths.map((entry, i) => ({ entry, link: row.entry_links[i] === true, location: locations.find(l => l.id === row.locations[i]) })).filter(entry => entry.location?.scope === "user") : [];
  const userScopeEntries: ExtensionSkillScopeEntry[] = userEntries.map(entry => ({
    path: entry.entry,
    link: entry.link,
    label: entry.location?.label ?? "",
    agents: entry.location ? EXTENSION_AGENTS.filter(agent => locationSupportsAgent(entry.location!, agent)) : [],
  }));
  const projects = projectScopes ?? [];
  const enabledProjects = projects.filter(project => project.enabled);
  const canConfigure = !!row && row.registered;
  const scopeActions = canConfigure
    ? <div className="extension-scope-actions">
      <button className="btn small" disabled={loading} onClick={() => setConfiguring(true)}>配置</button>
    </div>
    : null;

  return <div className="main extension-detail-page">
    <PageHeader title={<><Icon name={kind === "mcp" ? "mcp" : "skill"} /><span>{row?.name ?? initialTitle ?? EXTENSION_LABELS[kind]}</span></>} actions={<>
      {row && kind === "skill" && <button className="btn ghost icon-button danger" aria-label="卸载 Skill" title="卸载 Skill" disabled={busyAction === "uninstall" || loading} onClick={() => void openSkillUninstall()}><Icon name="trash" /></button>}
      {row && kind === "mcp" && <button className="btn ghost icon-button danger" aria-label="卸载 MCP" title="删除定义并移除所有作用域中的相同配置" disabled={busyAction === "uninstall" || loading} onClick={() => setRemoving("mcp")}><Icon name="trash" /></button>}
    </>} />
    {error && <div role="alert" className="extension-error">{error}</div>}
    {!row ? loading && <div className="small muted" role="status">正在读取资源…</div> : <div className="detail-layout">
      <div className="detail-main">
        <section className="card detail-section" aria-label="资源详情">
          <h2><Icon name={kind === "mcp" ? "settings" : "file"} />{kind === "mcp" ? "MCP 配置" : "Skill 内容"}</h2>
          {row.description && <p className="extension-description">{row.description}</p>}
          {row.error && <div className="extension-error">{row.error}</div>}
          {kind === "skill" ? <div className="extension-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children }) => <span>{children}</span>, img: () => null }}>{detail.content.replace(/^\uFEFF?---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")}</ReactMarkdown><details><summary className="small muted">查看 SKILL.md 原文</summary><pre className="extension-source">{detail.content}</pre></details></div>
            : <pre className="extension-source">{detail.content}</pre>}
        </section>
      </div>
      <aside className="detail-aside">
        {(kind === "skill" || !row.registered) && <section className="card detail-section" aria-label={kind === "skill" ? "源目录" : "配置文件"}>
          <h2><Icon name={kind === "skill" ? "folder" : "info"} />{kind === "skill" ? "源目录" : "配置文件"}</h2>
          <div className="extension-source-location">
            <span
              role="button"
              tabIndex={0}
              className="mono path-link"
              title={`${row.path} · 点击在文件管理器中打开`}
              onClick={openSourcePath}
              onKeyDown={e => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openSourcePath();
                }
              }}
            >{row.path}</span>
            {sourceKind && <span className="badge">{SOURCE_KIND_LABELS[sourceKind]}</span>}
          </div>
          {kind === "skill" && sourceKind && sourceKind !== "library" && <div className="extension-migration">
            <button className="btn small" disabled={loading} onClick={() => setRegistering(true)}>转移到 NoEnding</button>
            <span className="small muted">{sourceKind === "agent" ? "源目录位于 Agent 加载位置" : "源目录不在 NoEnding 源库中"}，转移后统一管理。</span>
          </div>}
          {kind === "mcp" && <div className="extension-migration">
            <button className="btn small" disabled={loading} onClick={() => setRegistering(true)}>登记</button>
            <span className="small muted">这是原生配置，登记后统一管理并可在用户级或项目中启停。</span>
          </div>}
        </section>}
        {row && <section className="card detail-section" aria-label="启用范围">
          <h2><Icon name="folder" />启用范围</h2>
          <p className="small muted">{!row.registered
            ? kind === "skill" ? "尚未转移，转移到 NoEnding 后可在用户级或项目中配置。" : "尚未登记，登记后可在用户级或项目中配置。"
            : "用户级对所有项目生效；项目级按项目添加。"}</p>
          {projectScopes === null ? <p className="small muted" role="status">正在读取启用范围…</p>
            : !userScopeEntries.length && !enabledProjects.length ? <p className="small muted">当前未在任何作用域启用。</p>
            : <dl className="extension-scope-summary">
              {userScopeEntries.length > 0 && <div className="extension-scope-summary-row">
                <dt>用户级{kind === "skill" && userScopeEntries.some(entry => !entry.link) && <span className="badge" title="该加载入口是真实目录而不是链接；NoEnding 只读，迁移或卸载时才会处理">物理入口</span>}</dt>
                <dd><span className="extension-scope-agents"><AgentIcons agents={scopeAgents(userScopeEntries)} /></span></dd>
              </div>}
              {enabledProjects.map(project => {
                const entries = project.workspaces.flatMap(workspace => workspace.entries);
                return <div className="extension-scope-summary-row" key={project.project_id}>
                  <dt>{project.project_name}{kind === "skill" && entries.some(entry => !entry.link) && <span className="badge" title="该加载入口是真实目录而不是链接；NoEnding 只读，迁移或卸载时才会处理">物理入口</span>}</dt>
                  <dd><span className="extension-scope-agents"><AgentIcons agents={scopeAgents(entries)} /></span></dd>
                </div>;
              })}
            </dl>}
          {scopeActions}
        </section>}
      </aside>
    </div>}
    {removing && row && <Modal title={removing === "skill" ? "卸载 Skill" : "卸载 MCP"} onClose={closeRemove}>
      {removing === "skill" ? <>
        <p>将移除该 Skill 在用户级和所有项目中的全部加载入口。</p>
        <label className="extension-check"><input type="checkbox" checked={deleteSource || deleteRequired} disabled={busyAction === "uninstall" || previewing || deleteRequired} onChange={e => setDeleteSource(e.target.checked)} />同时删除源目录</label>
        {deleteRequired && <p className="small muted">源目录位于 Agent 生效目录，必须删除源文件。</p>}
        <p className="extension-path mono">{preview?.source_path ?? row.path}</p>
      </> : <p>将删除该 MCP 定义，并移除所有作用域中完全相同的配置。不同配置或同名但内容不同的项会保留。</p>}
      {error && <div role="alert" className="extension-error">{error}</div>}
      <div className="row extension-dialog-actions"><button className="btn" disabled={busyAction === "uninstall"} onClick={closeRemove}>取消</button><button className="btn danger" disabled={busyAction === "uninstall" || (removing === "skill" && previewing)} onClick={() => {
        if (removing === "skill") void operation(() => api.uninstallExtensionSkill(row.id, row.revision, deleteSource || deleteRequired));
        else void operation(() => api.uninstallMcpDefinition(row.id));
      }}>{busyAction === "uninstall" ? "处理中…" : removing === "skill" && previewing ? "读取中…" : removing === "skill" ? deleteSource || deleteRequired ? "删除并卸载" : "仅移除" : "删除定义并移除"}</button></div>
    </Modal>}
    {configuring && row && <ExtensionConfigDialog resource={row} locations={locations}
      onClose={() => setConfiguring(false)} onDone={async () => { try { await refresh(true); } finally { setConfiguring(false); } }} />}
    {registering && row && (row.kind === "skill"
      ? <SkillRegisterDialog id={row.id} name={row.name} onClose={() => setRegistering(false)}
        onDone={result => {
          setRegistering(false);
          navigate({ view: "extension", resourceId: result.resource_id, kind: "skill", initialTitle: result.name });
        }} />
      : <McpRegisterDialog row={row} onClose={() => setRegistering(false)}
        onDone={() => { setRegistering(false); void refresh(true); }} />)}
  </div>;
}
