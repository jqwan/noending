import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import PageHeader from "../../layout/PageHeader";
import { AGENT_LABELS, type Agent, type PluginInventory, type PluginKind, type PluginLocation, type PluginResource, type PluginResourceDetail, type PluginScope, type Project, type WorkspacePath } from "../../types";
import "./plugins.css";
import { PLUGIN_AGENTS as AGENTS } from "./pluginAgents";

const LABELS: Record<PluginKind, string> = { skill: "Skills", mcp: "MCP", package: "插件包" };
const EMPTY: PluginInventory = { resources: [], locations: [], warnings: [] };
const scopeOf = (project: string, path: string): PluginScope => project ? { project_id: project, workspace_path_id: path } : {};

export default function PluginsView({ initialProjectId = "", initialAgent, initialTab = "skill" }: {
  initialProjectId?: string; initialAgent?: Agent; initialTab?: PluginKind;
}) {
  const [tab, setTab] = useState<PluginKind>(initialTab);
  const [project, setProject] = useState(initialProjectId);
  const [projects, setProjects] = useState<Project[]>([]);
  const [paths, setPaths] = useState<WorkspacePath[]>([]);
  const [path, setPath] = useState("");
  const [agent, setAgent] = useState<Agent | "">(initialAgent && AGENTS.includes(initialAgent) ? initialAgent : "");
  const [query, setQuery] = useState("");
  const [inherited, setInherited] = useState(false);
  const [inventory, setInventory] = useState<PluginInventory>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<PluginResourceDetail | null>(null);
  const [adding, setAdding] = useState(false);
  const [editor, setEditor] = useState<PluginResourceDetail | null>(null);
  const [remove, setRemove] = useState<{ resource: PluginResource; entry: string } | null>(null);
  const request = useRef(0);
  const scope = useMemo(() => scopeOf(project, path), [project, path]);
  const ready = !project || !!path;
  const activeScope = project ? "project" : "user";

  useEffect(() => { let live = true; void api.listProjects().then(rows => { if (live) setProjects(rows); }).catch(e => { if (live) setError(String(e)); }); return () => { live = false; }; }, []);
  useEffect(() => {
    let live = true;
    setPaths([]); setPath(""); setSelected(null); setInventory(EMPTY);
    if (project) void api.getProjectDetail(project).then(detail => {
      if (!live) return;
      setPaths(detail.workspace_paths); setPath(detail.workspace_paths.find(p => p.exists)?.id ?? "");
    }).catch(e => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [project]);

  const refresh = useCallback(async () => {
    const seq = ++request.current;
    if (!ready) { setLoading(false); return; }
    setLoading(true); setError("");
    try { const value = await api.listPluginResources(scope); if (seq === request.current) setInventory(value); }
    catch (e) { if (seq === request.current) { setError(String(e)); setInventory(EMPTY); } }
    finally { if (seq === request.current) setLoading(false); }
  }, [ready, scope]);
  useEffect(() => { setSelected(null); setAdding(false); setEditor(null); setRemove(null); void refresh(); return () => { ++request.current; }; }, [refresh]);

  const locationsFor = (row: PluginResource) => row.locations.flatMap(id => inventory.locations.find(l => l.id === id) ?? []);
  const belongs = (row: PluginResource) => locationsFor(row).some(l => l.scope === activeScope);
  const rows = inventory.resources.filter(row => row.kind === tab
    && (belongs(row) || (project && inherited))
    && (!agent || locationsFor(row).some(l => l.agent === agent || (!l.agent && (agent === "codex" || agent === "pi" || l.scope === "project" && agent === "antigravity")))))
    .filter(row => `${row.name} ${row.description} ${row.path}`.toLowerCase().includes(query.toLowerCase()));
  const destinations = inventory.locations.filter(l => l.kind === tab && l.scope === activeScope && l.writable && !l.package && (!agent || l.agent === agent || l.agent === null));

  const operation = async (work: () => Promise<{ message: string }>, keepId?: string) => {
    setBusy(true); setError("");
    try { const result = await work(); showToast(result.message); setRemove(null); setSelected(null); await refresh(); if (keepId) setSelected(await api.getPluginResource(scope, keepId)); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const inspect = async (row: PluginResource) => {
    const seq = request.current;
    setBusy(true); setError("");
    try { const detail = await api.getPluginResource(scope, row.id); if (seq === request.current) setSelected(detail); }
    catch (e) { if (seq === request.current) setError(String(e)); }
    finally { setBusy(false); }
  };
  const packageAction = (row: PluginResource, action: string) => {
    const loc = locationsFor(row)[0];
    if (loc?.agent) void operation(() => api.managePluginPackage(scope, loc.agent!, action, row.name, row.id, row.revision), action === "remove" || action === "inherit" ? undefined : row.id);
  };
  const projectMcpOverride = (detail: PluginResourceDetail, enabled: boolean) => {
    const source = locationsFor(detail.resource)[0];
    const destination = inventory.locations.find(l => l.kind === "mcp" && l.scope === "project" && l.agent === source?.agent && l.writable && !l.label.includes("私有"));
    if (!destination || !detail.config) return;
    const config = { ...detail.config, [source.agent === "antigravity" ? "disabled" : "enabled"]: source.agent === "antigravity" ? !enabled : enabled };
    void operation(async () => api.savePluginMcp(scope, destination.id, await api.pluginLocationRevision(scope, destination.id), detail.resource.name, config, true));
  };

  return <div className="main board-page plugins-page">
    <PageHeader title="插件" actions={<>
      <button className="btn ghost icon-button" aria-label="刷新插件" title="刷新插件" disabled={busy || loading || !ready} onClick={() => void refresh()}><Icon name="refresh" className={loading ? "spinning" : ""} /></button>
      <button className="btn ghost icon-button" aria-label={tab === "mcp" ? "新增 MCP" : tab === "skill" ? "安装 Skill" : "安装插件包"} title={tab === "mcp" ? "新增 MCP" : "安装"} disabled={busy || loading || !ready} onClick={() => setAdding(true)}><Icon name="plus" /></button>
    </>} />
    <div className="board-toolbar plugins-toolbar">
      <div className="plugin-tabs" role="tablist" aria-label="插件类别">{(Object.keys(LABELS) as PluginKind[]).map(kind => <button role="tab" disabled={busy} aria-selected={tab === kind} className={tab === kind ? "active" : ""} key={kind} onClick={() => { setTab(kind); setSelected(null); }}>{LABELS[kind]}</button>)}</div>
      <div className="plugin-filters">
        <select aria-label="配置范围" value={project} disabled={busy} onChange={e => { setPath(""); setProject(e.target.value); }}><option value="">用户级 · 所有项目</option>{projects.map(p => <option key={p.id} value={p.id}>项目级 · {p.name}{p.visibility === "archived" ? "（已归档）" : ""}</option>)}</select>
        {project && <select aria-label="配置工作目录" value={path} disabled={busy} onChange={e => setPath(e.target.value)}><option value="" disabled>选择工作目录</option>{paths.map(p => <option key={p.id} value={p.id} disabled={!p.exists}>{p.canonical_path}{!p.exists ? "（目录缺失）" : ""}</option>)}</select>}
        <select aria-label="筛选 Agent" value={agent} disabled={busy} onChange={e => setAgent(e.target.value as Agent | "")}><option value="">全部 Agent</option>{AGENTS.map(a => <option key={a} value={a}>{AGENT_LABELS[a]}</option>)}</select>
        <input type="text" aria-label="搜索插件" placeholder="搜索名称、说明或路径…" value={query} onChange={e => setQuery(e.target.value)} />
        {project && <label className="plugin-check"><input type="checkbox" checked={inherited} onChange={e => setInherited(e.target.checked)} />显示用户级资源</label>}
      </div>
    </div>
    <p className="small muted plugin-note">{project ? "配置写入所选工作目录；其他目录与 worktree 不会自动同步。原生继承及项目信任规则仍由 Agent 处理。" : "管理本机已有配置。共享目录中的 Skill 会被支持该路径的 Agent 发现。"} 配置变更后，已有终端请按 Agent 的方式重新加载或重新打开。</p>
    {error && <div role="alert" className="plugin-error">{error}</div>}
    {inventory.warnings.length > 0 && <details className="plugin-warnings"><summary>部分来源无法读取（{inventory.warnings.length}）</summary>{inventory.warnings.map((warning, i) => <p key={i}>{warning}</p>)}</details>}
    {!ready ? <div className="empty">请选择可用的工作目录。</div> : loading ? <div className="muted small">正在读取本地资源…</div> : <div className="plugin-layout">
      <section className="plugin-list" aria-label={LABELS[tab]}>{!rows.length && <div className="empty">没有匹配的{LABELS[tab]}。</div>}{rows.map(row => {
        const locations = locationsFor(row);
        const own = belongs(row);
        return <button key={row.id} className={`plugin-row${selected?.resource.id === row.id ? " selected" : ""}`} onClick={() => void inspect(row)} disabled={busy}>
          <span className="plugin-row-heading"><Icon name={row.kind === "mcp" ? "sliders" : row.kind === "package" ? "plugin" : "spark"} /><strong>{row.name}</strong>{row.error && <span className="badge warn">配置异常</span>}{row.managed && <span className="badge">受管资源</span>}{row.enabled !== null && <span className={`badge ${row.enabled ? "accent" : ""}`}>{row.enabled ? "已启用" : "已禁用"}</span>}</span>
          {row.description && <span className="plugin-description">{row.description}</span>}
          <span className="small muted">{[...new Set(locations.map(l => l.package ? `由 ${l.package} 提供` : l.label))].join(" · ")}{!own ? " · 用户级" : ""}</span>
          <span className="plugin-path mono">{row.path}</span>
        </button>;
      })}</section>
      {selected ? <aside className="card plugin-detail" aria-label="资源详情">
        <div className="row between"><h2>{selected.resource.name}</h2><button className="btn ghost icon-button" title="关闭详情" aria-label="关闭详情" onClick={() => setSelected(null)}><Icon name="close" /></button></div>
        <p className="mono plugin-path">{selected.resource.path}</p>
        <div className="row plugin-detail-actions"><button className="btn small" onClick={() => void api.openPath(selected.resource.path).catch(e => setError(String(e)))}>打开所在目录</button>
          {project && !belongs(selected.resource) && !selected.resource.managed && (() => {
            const loc = locationsFor(selected.resource)[0];
            if (selected.resource.kind === "package" && (loc?.agent === "codex" || loc?.agent === "claude_code")) return <><button className="btn small" disabled={busy} onClick={() => packageAction(selected.resource, "project_enable")}>在当前目录启用</button><button className="btn small" disabled={busy} onClick={() => packageAction(selected.resource, "project_disable")}>在当前目录禁用</button></>;
            if (selected.resource.kind === "mcp" && loc?.agent && loc.agent !== "claude_code") return <><button className="btn small" disabled={busy} onClick={() => projectMcpOverride(selected, true)}>在当前目录启用</button><button className="btn small" disabled={busy} onClick={() => projectMcpOverride(selected, false)}>在当前目录禁用</button></>;
            return null;
          })()}
          {selected.resource.kind === "mcp" && !selected.resource.managed && belongs(selected.resource) && <button className="btn small" disabled={busy} onClick={() => setEditor(selected)}>编辑配置</button>}
          {selected.resource.kind === "mcp" && !selected.resource.managed && belongs(selected.resource) && selected.resource.enabled !== null && (() => {
            const loc = locationsFor(selected.resource)[0];
            if (!loc || loc.agent === "claude_code" || !selected.config) return null;
            return <button className="btn small" disabled={busy} onClick={() => {
              const config = { ...selected.config, [loc.agent === "antigravity" ? "disabled" : "enabled"]: loc.agent === "antigravity" ? selected.resource.enabled : !selected.resource.enabled };
              void operation(() => api.savePluginMcp(scope, loc.id, selected.resource.revision, selected.resource.name, config, false), selected.resource.id);
            }}>{selected.resource.enabled ? "禁用" : "启用"}</button>;
          })()}
          {selected.resource.kind === "package" && !selected.resource.managed && belongs(selected.resource) && (() => {
            const loc = locationsFor(selected.resource)[0];
            return <>{loc?.agent !== "pi" && <button className="btn small" disabled={busy} onClick={() => packageAction(selected.resource, selected.resource.enabled === true ? "disable" : "enable")}>{selected.resource.enabled === true ? "禁用" : "启用"}</button>}
              {loc?.scope === "project" && (loc.agent === "codex" || loc.agent === "claude_code") && <button className="btn small" disabled={busy} onClick={() => packageAction(selected.resource, "inherit")}>恢复继承</button>}
              {(loc?.scope === "user" || loc?.agent === "claude_code" || loc?.agent === "pi") && <button className="btn small ghost danger" disabled={busy} onClick={() => setRemove({ resource: selected.resource, entry: "" })}>卸载</button>}</>;
          })()}
          {selected.resource.kind === "mcp" && !selected.resource.managed && belongs(selected.resource) && <button className="btn small ghost danger" disabled={busy} onClick={() => setRemove({ resource: selected.resource, entry: "" })}>删除配置</button>}
        </div>
        {selected.resource.kind === "skill" && <>
          {selected.resource.skill_controls.filter(c => c.scope === activeScope).map(control => <div className="row plugin-detail-actions" key={`${control.agent}:${control.scope}`}>
            <span className="small muted">{AGENT_LABELS[control.agent]} · {control.enabled === null ? "本层未设置启停" : control.enabled ? "本层已启用" : "本层已禁用"}</span>
            <button className="btn small" disabled={busy} onClick={() => void operation(() => api.setPluginSkillState(scope, selected.resource.id, control.scope, control.revision, control.enabled === false), selected.resource.id)}>{control.enabled === false ? "启用" : "禁用"}</button>
            {control.enabled !== null && <button className="btn small" disabled={busy} onClick={() => void operation(() => api.setPluginSkillState(scope, selected.resource.id, control.scope, control.revision, null), selected.resource.id)}>恢复继承</button>}
          </div>)}
          <p className="small muted">Skill 内容只读。下方列出它的加载入口，移除共享源目录会影响所有使用它的 Agent。</p>
          <div className="plugin-entries">{selected.resource.entry_paths.map((entry, i) => {
            const loc = inventory.locations.find(l => l.id === selected.resource.locations[i]);
            return <div key={`${loc?.id}:${entry}`} className="plugin-entry"><span className="small"><strong>{loc?.label}</strong><span className="plugin-path mono">{entry}</span></span>{loc?.writable && !loc.package && loc.scope === activeScope && <button className="btn small ghost danger" disabled={busy} onClick={() => setRemove({ resource: selected.resource, entry })}>移除</button>}</div>;
          })}</div>
        </>}
        {selected.resource.kind === "skill" ? <div className="plugin-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children }) => <span>{children}</span>, img: () => null }}>{selected.content.replace(/^\uFEFF?---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")}</ReactMarkdown><details><summary className="small muted">查看 SKILL.md 原文</summary><pre className="plugin-source">{selected.content}</pre></details></div> : <pre className="plugin-source">{selected.content || "此项由原生 CLI 管理，可在其插件管理界面查看安装详情。"}</pre>}
      </aside> : <aside className="plugin-placeholder"><Icon name="plugin" /><p>选择一项查看来源和配置</p></aside>}
    </div>}
    {(adding || editor) && <InstallDialog kind={editor ? "mcp" : tab} scope={scope} scopeName={activeScope} paths={paths} locations={editor ? inventory.locations.filter(l => editor.resource.locations.includes(l.id)) : destinations} agent={agent || "codex"} detail={editor} onClose={() => { if (!busy) { setAdding(false); setEditor(null); void refresh(); } }} onDone={() => { setAdding(false); setEditor(null); setSelected(null); void refresh(); }} />}
    {remove && <Modal title={remove.resource.kind === "package" ? "卸载插件包" : "移除资源"} onClose={() => { if (!busy) setRemove(null); }}>
      <p>将移除 <strong>{remove.resource.name}</strong>。</p><p className="small muted">{remove.resource.kind === "skill" ? "链接入口只移除链接；普通 Skill 目录会删除其中全部文件。若其他入口指向这个源目录，也会受到影响。" : remove.resource.kind === "package" ? "由对应 Agent 的原生卸载命令执行，包内资源一并受影响。" : "只删除这一项 MCP 配置，保留文件中的其他配置。"}</p>
      {remove.entry && <p className="mono plugin-path">{remove.entry}</p>}
      {error && <div role="alert" className="plugin-error">{error}</div>}
      <div className="row" style={{ justifyContent: "flex-end" }}><button className="btn" disabled={busy} onClick={() => setRemove(null)}>取消</button><button className="btn danger" disabled={busy} onClick={() => {
        const { resource: r, entry } = remove;
        if (r.kind === "skill") void operation(() => api.removePluginSkill(scope, r.id, entry, r.revision));
        else if (r.kind === "mcp") void operation(() => api.removePluginMcp(scope, r.id, r.revision));
        else packageAction(r, "remove");
      }}>{busy ? "处理中…" : "确认移除"}</button></div>
    </Modal>}
  </div>;
}

function InstallDialog({ kind, scope, scopeName, paths, locations, agent: initialAgent, detail, onClose, onDone }: {
  kind: PluginKind; scope: PluginScope; scopeName: string; locations: PluginLocation[]; agent: Agent;
  paths: WorkspacePath[]; detail: PluginResourceDetail | null; onClose: () => void; onDone: () => void;
}) {
  const [locationId, setLocationId] = useState(locations[0]?.id ?? "");
  const [agent, setAgent] = useState<Agent>(initialAgent);
  const [source, setSource] = useState("");
  const [installMode, setInstallMode] = useState<"copy" | "link">("copy");
  const [name, setName] = useState(detail?.resource.name ?? "");
  const [draft, setDraft] = useState(detail?.content ?? '{\n  "command": "",\n  "args": []\n}');
  const [revision, setRevision] = useState(detail?.resource.revision ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [extraPaths, setExtraPaths] = useState<string[]>([]);
  const [outcomes, setOutcomes] = useState<Record<string, { ok: boolean; text: string }>>({});
  const editingLocked = busy || Object.keys(outcomes).length > 0;
  const location = locations.find(l => l.id === locationId);
  const effectiveAgent = location?.agent ?? agent;
  useEffect(() => {
    if (kind !== "mcp" || detail || !locationId) return;
    let live = true; setRevision("");
    void api.pluginLocationRevision(scope, locationId).then(r => { if (live) setRevision(r); }).catch(e => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [kind, detail, locationId, scope]);
  const parsed = useMemo(() => { try { const value = JSON.parse(draft); return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null; } catch { return null; } }, [draft]);
  const update = (key: string, value: unknown) => { if (parsed) setDraft(JSON.stringify({ ...parsed, [key]: value }, null, 2)); };
  const remoteKey = effectiveAgent === "antigravity" ? "serverUrl" : "url";
  const remote = !!parsed && ("url" in parsed || "serverUrl" in parsed);
  const supported = kind !== "package" || scopeName === "user" || agent === "claude_code" || agent === "pi";
  const submit = async () => {
    setBusy(true); setError("");
    try {
      if (kind === "package") {
        const result = await api.managePluginPackage(scope, agent, "install", source);
        showToast(result.message); onDone(); return;
      }
      const targets = [scope.workspace_path_id ?? "user", ...extraPaths];
      let failed = false;
      for (const [index, target] of targets.entries()) {
        if (outcomes[target]?.ok) continue;
        try {
          const targetScope = index === 0 ? scope : { project_id: scope.project_id, workspace_path_id: target };
          let destinationId = locationId;
          let expected = revision;
          if (index > 0) {
            const list = await api.listPluginResources(targetScope);
            const dest = list.locations.find(l => l.scope === "project" && l.writable && !l.package && l.kind === kind && l.agent === location?.agent && l.label === location?.label);
            if (!dest) throw new Error("目录中没有对应的配置位置");
            destinationId = dest.id;
            if (kind === "mcp") expected = await api.pluginLocationRevision(targetScope, destinationId);
          }
          const result = kind === "skill" ? await api.installPluginSkill(targetScope, source, destinationId, installMode)
            : await api.savePluginMcp(targetScope, destinationId, expected, name, parsed!, index > 0 || !detail);
          setOutcomes(current => ({ ...current, [target]: { ok: true, text: result.message } }));
        } catch (e) { failed = true; setOutcomes(current => ({ ...current, [target]: { ok: false, text: String(e) } })); }
      }
      if (!failed) { showToast("配置操作已完成；已有终端请重新加载或重新打开"); onDone(); }
      else setError("部分目录操作失败，成功项已保留。重试只处理失败项，或关闭后刷新列表。");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <Modal title={kind === "mcp" ? detail ? "编辑 MCP 配置" : "新增 MCP" : kind === "skill" ? "安装 Skill" : "安装插件包"} wide onClose={() => { if (!busy) onClose(); }}>
    <div className="plugin-form">
      {kind !== "package" ? <label>配置位置<select aria-label="配置位置" value={locationId} disabled={busy || !!detail || Object.keys(outcomes).length > 0} onChange={e => setLocationId(e.target.value)}>{locations.map(l => <option key={l.id} value={l.id}>{l.label} · {l.path}</option>)}</select></label> : <label>Agent<select aria-label="安装到 Agent" value={agent} disabled={editingLocked} onChange={e => setAgent(e.target.value as Agent)}>{AGENTS.map(a => <option key={a} value={a}>{AGENT_LABELS[a]}</option>)}</select></label>}
      {!detail && kind !== "package" && scope.project_id && paths.some(p => p.exists && p.id !== scope.workspace_path_id) && <fieldset className="plugin-targets"><legend>同时新增到其他目录（可选，不覆盖同名项）</legend>{paths.filter(p => p.exists && p.id !== scope.workspace_path_id).map(p => <label className="plugin-check" key={p.id}><input type="checkbox" checked={extraPaths.includes(p.id)} disabled={busy || Object.keys(outcomes).length > 0} onChange={e => setExtraPaths(current => e.target.checked ? [...current, p.id] : current.filter(id => id !== p.id))} />{p.canonical_path}</label>)}</fieldset>}
      {kind === "skill" && <><label>安装方式<select aria-label="Skill 安装方式" value={installMode} disabled={editingLocked} onChange={e => setInstallMode(e.target.value as "copy" | "link")}><option value="copy">复制目录 · 独立安装</option><option value="link">链接目录 · 共用源文件</option></select></label><label>源 Skill 目录<div className="row"><input type="text" aria-label="源 Skill 目录" value={source} onChange={e => setSource(e.target.value)} disabled={busy} /><button className="btn" disabled={busy} onClick={async () => { const picked = await open({ directory: true, multiple: false, title: "选择包含 SKILL.md 的目录" }); if (typeof picked === "string") setSource(picked); }}>选择目录</button></div></label><p className="small muted">{installMode === "copy" ? "复制整个 Skill 目录，保留原文件。" : "建立目录链接，源文件修改会影响所有使用它的入口；请保留源目录。"}共享安装会被支持该路径的 Agent 发现，不会覆盖已有目录。</p></>}
      {kind === "package" && <><label>插件目标<input type="text" aria-label="插件目标" placeholder={agent === "pi" ? "npm:包名、git:地址或本地目录" : agent === "antigravity" ? "本地插件目录或原生 CLI 支持的目标" : "插件名@已配置的市场"} value={source} disabled={editingLocked} onChange={e => setSource(e.target.value)} /></label><p className="small muted">调用 Agent 原生安装命令。插件市场、账号认证及安装依赖由 Agent 管理。</p>{!supported && <p role="alert" className="plugin-error">此 Agent 的安装入口只支持用户级，请切换到用户级安装。</p>}</>}
      {kind === "mcp" && <>
        <label>名称<input type="text" aria-label="MCP 名称" value={name} disabled={editingLocked || !!detail} onChange={e => setName(e.target.value)} /></label>
        {parsed && <><label>连接方式<select aria-label="连接方式" value={remote ? "http" : "stdio"} disabled={editingLocked} onChange={e => {
          const next = { ...parsed }; delete next.command; delete next.args; delete next.url; delete next.serverUrl; delete next.type;
          if (e.target.value === "http") { next[remoteKey] = "https://"; if (effectiveAgent === "claude_code") next.type = "http"; }
          else { next.command = ""; next.args = []; }
          setDraft(JSON.stringify(next, null, 2));
        }}><option value="stdio">本地进程 · stdio</option><option value="http">远程服务 · HTTP</option></select></label>
          {remote ? <label>服务地址<input type="text" aria-label="MCP 服务地址" value={String(parsed[remoteKey] ?? "")} disabled={editingLocked} onChange={e => update(remoteKey, e.target.value)} /></label> : <><label>启动程序<input type="text" aria-label="MCP 启动程序" value={String(parsed.command ?? "")} disabled={editingLocked} onChange={e => update("command", e.target.value)} /></label><label>启动参数（每行一个）<textarea aria-label="MCP 启动参数" rows={3} value={Array.isArray(parsed.args) ? parsed.args.join("\n") : ""} disabled={editingLocked} onChange={e => update("args", e.target.value ? e.target.value.split("\n") : [])} /></label></>}
        </>}
        <details className="plugin-advanced"><summary>高级配置 · 环境变量、请求头及 JSON</summary><label>完整服务器配置 · JSON<textarea aria-label="MCP JSON 配置" className="mono plugin-json" rows={10} value={draft} disabled={editingLocked} spellCheck={false} onChange={e => setDraft(e.target.value)} /></label></details>
        <p className="small muted">环境变量、请求头及 Agent 专用选项可在 JSON 中配置。只修改当前服务器，保留其他配置；上一个文件版本保存为同目录备份。保存不启动或连接服务器。</p>
      </>}
      {error && <div role="alert" className="plugin-error">{error}</div>}
      {Object.entries(outcomes).map(([id, result]) => <p className="small plugin-path" key={id}>{paths.find(p => p.id === id)?.canonical_path ?? "用户级"} · {result.ok ? "成功" : "失败"}：{result.text}</p>)}
      <div className="row" style={{ justifyContent: "flex-end" }}><button className="btn" disabled={busy} onClick={onClose}>取消</button><button className="btn primary" disabled={busy || !supported || (kind === "mcp" ? !locationId || !revision || !name.trim() || !parsed : !source.trim() || kind === "skill" && !locationId)} onClick={() => void submit()}>{busy ? "处理中…" : kind === "mcp" ? "保存" : "安装"}</button></div>
    </div>
  </Modal>;
}
