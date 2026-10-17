import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import PageHeader from "../../layout/PageHeader";
import { preferenceValues, useViewPreference, useViewScroll, useViewState } from "../../hooks/useViewState";
import { AGENT_LABELS, type Agent, type ExtensionInventory, type ExtensionKind, type ExtensionScope, type Project, type WorkspacePath } from "../../types";
import { EXTENSION_AGENTS as AGENTS } from "./extensionAgents";
import { agentNames, locationSupportsAgent, EXTENSION_LABELS as LABELS, extensionDetailRoute, resourceLocations, resourceSource } from "./extensionPresentation";
import ExtensionInstallDialog from "./ExtensionInstallDialog";
import "./extensions.css";

const EMPTY: ExtensionInventory = { resources: [], locations: [], warnings: [] };

export default function ExtensionsView({ initialProjectId = "", initialAgent, initialTab = "skill", navigate }: {
  initialProjectId?: string; initialAgent?: Agent; initialTab?: ExtensionKind; navigate: (route: Route) => void;
}) {
  const stateKey = `extensions.${initialProjectId}:${initialAgent ?? ""}:${initialTab}`;
  const [tab, setTab] = useViewState<ExtensionKind>(`${stateKey}.tab`, initialTab);
  const [project, setProject] = useViewState(`${stateKey}.project`, initialProjectId);
  const [path, setPath] = useViewState(`${stateKey}.path`, "");
  const [agent, setAgent] = useViewState<Agent | "">(`${stateKey}.agent`, initialAgent && AGENTS.includes(initialAgent) ? initialAgent : "");
  const [query, setQuery] = useViewState(`${stateKey}.query`, "");
  const [inherited, setInherited] = useViewState(`${stateKey}.inherited`, false);
  const [viewMode, setViewMode] = useViewPreference<"cards" | "list">("extensions.viewMode", "cards", preferenceValues("cards", "list"));
  const [projects, setProjects] = useState<Project[]>([]);
  const [paths, setPaths] = useState<WorkspacePath[]>([]);
  const [inventory, setInventory] = useState<ExtensionInventory>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState(false);
  const request = useRef(0);
  const scope: ExtensionScope = useMemo(() => project ? { project_id: project, workspace_path_id: path } : {}, [project, path]);
  const ready = !project || paths.some(p => p.id === path && p.exists);
  const activeScope = project ? "project" : "user";

  useEffect(() => { let live = true; void api.listProjects().then(rows => { if (live) setProjects(rows); }).catch(e => { if (live) setError(String(e)); }); return () => { live = false; }; }, []);
  useEffect(() => {
    let live = true;
    setPaths([]); setInventory(EMPTY);
    if (project) void api.getProjectDetail(project).then(detail => {
      if (!live) return;
      setPaths(detail.workspace_paths);
      setPath(current => detail.workspace_paths.some(p => p.id === current && p.exists) ? current : detail.workspace_paths.find(p => p.exists)?.id ?? "");
    }).catch(e => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [project]);

  const refresh = useCallback(async () => {
    const seq = ++request.current;
    if (!ready) { setLoading(false); return; }
    setLoading(true); setError("");
    try { const value = await api.listExtensionResources(scope); if (seq === request.current) setInventory(value); }
    catch (e) { if (seq === request.current) { setError(String(e)); setInventory(EMPTY); } }
    finally { if (seq === request.current) setLoading(false); }
  }, [ready, scope]);
  useEffect(() => { setAdding(false); void refresh(); return () => { ++request.current; }; }, [refresh]);

  const rows = inventory.resources.filter(row => {
    const locations = resourceLocations(inventory, row);
    return row.kind === tab && (locations.some(l => l.scope === activeScope) || project && inherited)
      && (!agent || locations.some(l => locationSupportsAgent(l, agent)))
      && `${row.name} ${row.description} ${row.path}`.toLowerCase().includes(query.toLowerCase());
  });
  const destinations = inventory.locations.filter(l => l.kind === tab && l.scope === activeScope && l.writable && !l.package && (!agent || l.agent === agent || l.agent === null));
  const scrollRef = useViewScroll(`${stateKey}.scroll`, !loading);

  return <div className="main board-page extensions-page" ref={scrollRef}>
    <PageHeader title="扩展" actions={<>
      <button className="btn ghost icon-button" aria-label="刷新扩展" title="刷新扩展" disabled={loading || !ready} onClick={() => void refresh()}><Icon name="refresh" className={loading ? "spinning" : ""} /></button>
      <button className="btn ghost icon-button" aria-label={tab === "mcp" ? "新增 MCP" : tab === "skill" ? "安装 Skill" : "安装插件包"} title={tab === "mcp" ? "新增 MCP" : "安装"} disabled={loading || !ready} onClick={() => setAdding(true)}><Icon name="plus" /></button>
    </>} />
    <div className="board-toolbar extensions-toolbar">
      <div className="extension-tabs" role="tablist" aria-label="扩展类别">{(Object.keys(LABELS) as ExtensionKind[]).map(kind => <button role="tab" aria-selected={tab === kind} key={kind} onClick={() => setTab(kind)}>{LABELS[kind]}</button>)}</div>
      <div className="extension-filters">
        <select aria-label="配置范围" value={project} onChange={e => { setPath(""); setProject(e.target.value); }}><option value="">用户级 · 所有项目</option>{projects.map(p => <option key={p.id} value={p.id}>项目级 · {p.name}{p.visibility === "archived" ? "（已归档）" : ""}</option>)}</select>
        {project && <select aria-label="配置工作目录" value={path} onChange={e => setPath(e.target.value)}><option value="" disabled>选择工作目录</option>{paths.map(p => <option key={p.id} value={p.id} disabled={!p.exists}>{p.canonical_path}{!p.exists ? "（目录缺失）" : ""}</option>)}</select>}
        <select aria-label="筛选 Agent" value={agent} onChange={e => setAgent(e.target.value as Agent | "")}><option value="">全部 Agent</option>{AGENTS.map(a => <option key={a} value={a}>{AGENT_LABELS[a]}</option>)}</select>
        <input type="text" aria-label="搜索扩展" placeholder="搜索名称、说明或路径…" value={query} onChange={e => setQuery(e.target.value)} />
        {project && <label className="extension-check"><input type="checkbox" checked={inherited} onChange={e => setInherited(e.target.checked)} />显示用户级资源</label>}
      </div>
      <div className="toolbar ws-controls"><span className="small muted">共 {rows.length} 项</span>
        <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
          <button className={viewMode === "cards" ? "on" : ""} aria-pressed={viewMode === "cards"} aria-label="卡片视图" title="卡片视图" onClick={() => setViewMode("cards")}><Icon name="grid" /></button>
          <button className={viewMode === "list" ? "on" : ""} aria-pressed={viewMode === "list"} aria-label="列表视图" title="列表视图" onClick={() => setViewMode("list")}><Icon name="list" /></button>
        </div>
      </div>
    </div>
    {error && <div role="alert" className="extension-error">{error}</div>}
    {inventory.warnings.length > 0 && <details className="extension-warnings"><summary>部分来源无法读取（{inventory.warnings.length}）</summary>{inventory.warnings.map((warning, i) => <p key={i}>{warning}</p>)}</details>}
    {!ready ? <div className="empty">请选择可用的工作目录。</div> : loading ? <div className="muted small" role="status">正在读取本地资源…</div> : <section className={viewMode === "cards" ? "ws-grid board-grid extension-cards" : "extension-list"} aria-label={LABELS[tab]}>
      {!rows.length && <div className="empty">没有匹配的{LABELS[tab]}。</div>}{rows.map(row => {
        const locations = resourceLocations(inventory, row);
        return <button key={row.id} className={`extension-row extension-${viewMode}`} onClick={() => navigate(extensionDetailRoute(row, project || undefined, project ? path : undefined))}>
          <span className="extension-row-heading"><Icon name={row.kind === "mcp" ? "sliders" : row.kind === "package" ? "plugin" : "spark"} /><strong>{row.name}</strong>{row.error && <span className="badge warn">配置异常</span>}{row.enabled !== null && <span className={`badge ${row.enabled ? "accent" : ""}`}>{row.enabled ? "已启用" : "已禁用"}</span>}</span>
          {row.description && <span className="extension-description">{row.description}</span>}
          <span className="small muted extension-row-meta">{resourceSource(row, locations)} · {agentNames(locations)} · {locations.some(l => l.scope === "project") ? "项目级" : "用户级"}</span>
          <span className="extension-path mono" title={row.path}>{row.path}</span>
        </button>;
      })}
    </section>}
    {adding && <ExtensionInstallDialog kind={tab} scope={scope} scopeName={activeScope} paths={paths} locations={destinations} agent={agent || "codex"} detail={null} onClose={() => { setAdding(false); void refresh(); }} onDone={() => { setAdding(false); void refresh(); }} />}
  </div>;
}
