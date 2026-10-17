import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import PageHeader from "../../layout/PageHeader";
import { AGENT_LABELS, type ExtensionInventory, type ExtensionKind, type ExtensionResourceDetail, type ExtensionScope } from "../../types";
import ExtensionInstallDialog from "./ExtensionInstallDialog";
import { EXTENSION_LABELS, extensionDetailRoute, resourceAgents, resourceLocations, resourceSource } from "./extensionPresentation";
import "./extensions.css";

const USER_SCOPE: ExtensionScope = {};
const EMPTY: ExtensionInventory = { resources: [], locations: [], warnings: [] };

export default function ExtensionDetailView({ resourceId, kind, initialTitle, projectId, workspacePathId, navigate, goBack }: {
  resourceId: string; kind: ExtensionKind; initialTitle?: string; projectId?: string; workspacePathId?: string;
  navigate: (route: Route) => void; goBack: (fallback?: Route) => void;
}) {
  const scope = useMemo<ExtensionScope>(() => projectId ? { project_id: projectId, workspace_path_id: workspacePathId } : {}, [projectId, workspacePathId]);
  const [inventory, setInventory] = useState<ExtensionInventory>(EMPTY);
  const [detail, setDetail] = useState<ExtensionResourceDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editing, setEditing] = useState(false);
  const [removeEntry, setRemoveEntry] = useState<string | null>(null);
  const request = useRef(0);
  const alive = useRef(true);
  const pending = useRef(false);
  const board: Route = { view: "extensions", projectId, tab: kind };

  const refresh = useCallback(async () => {
    const seq = ++request.current;
    setLoading(true);
    try {
      const list = await api.listExtensionResources(scope);
      const exists = list.resources.some(r => r.id === resourceId);
      const value = exists ? await api.getExtensionResource(scope, resourceId) : null;
      if (seq !== request.current || !alive.current) return exists;
      setInventory(list); setDetail(value); setError(exists ? "" : "此资源已移除或来源已变化，请返回扩展看板。");
      return exists;
    } catch (e) {
      if (seq === request.current && alive.current) { setDetail(null); setError(String(e)); }
      throw e;
    } finally { if (seq === request.current && alive.current) setLoading(false); }
  }, [resourceId, scope]);
  useEffect(() => {
    alive.current = true;
    void refresh().catch(() => {});
    return () => { alive.current = false; ++request.current; };
  }, [refresh]);

  const operation = async (work: () => Promise<{ message: string }>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try {
      const result = await work();
      if (!alive.current) return;
      showToast(result.message); setRemoveEntry(null);
      if (!await refresh() && alive.current) navigate(board);
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  const row = detail?.resource;
  const locations = row ? resourceLocations(inventory, row) : [];
  const userLocations = locations.filter(l => l.scope === "user");
  const userLocation = userLocations[0];
  const editable = !!row && !row.managed && !!userLocation;
  const source = row ? resourceSource(row, locations) : "";
  const packageAction = (action: string) => {
    if (!row || !userLocation?.agent || !editable) return;
    void operation(() => api.managePluginPackage(USER_SCOPE, userLocation.agent!, action, row.name, row.id, row.revision));
  };
  const canRemove = editable && (kind !== "skill" || userLocations.some(l => l.writable && !l.package));
  const packages = row ? inventory.resources.filter(r => r.kind === "package" && locations.some(l => l.package === r.name
    && resourceLocations(inventory, r).some(parent => parent.agent === l.agent && parent.scope === l.scope))) : [];
  const children = row && kind === "package" ? inventory.resources.filter(r => r.kind !== "package"
    && resourceLocations(inventory, r).some(l => l.package === row.name && locations.some(parent => parent.agent === l.agent && parent.scope === l.scope))) : [];

  return <div className="main extension-detail-page">
    <PageHeader title={row?.name ?? initialTitle ?? EXTENSION_LABELS[kind]} back="扩展" onBack={() => goBack(board)} actions={<>
      <button className="btn ghost icon-button" aria-label="刷新资源" title="刷新资源" disabled={busy || loading} onClick={() => void refresh().catch(() => {})}><Icon name="refresh" className={loading ? "spinning" : ""} /></button>
      {row && <button className="btn ghost icon-button" aria-label="打开所在目录" title="打开所在目录" onClick={() => void api.openPath(row.path).catch(e => setError(String(e)))}><Icon name="folder" /></button>}
      {editable && kind === "mcp" && <button className="btn ghost icon-button" aria-label="编辑配置" title="编辑用户级配置" disabled={busy || loading} onClick={() => setEditing(true)}><Icon name="edit" /></button>}
      {canRemove && kind !== "skill" && <button className="btn ghost icon-button danger" aria-label={kind === "package" ? "卸载" : "删除配置"} title={kind === "package" ? "卸载插件包" : "删除用户级配置"} disabled={busy || loading} onClick={() => setRemoveEntry("")}><Icon name="trash" /></button>}
    </>} />
    {error && <div role="alert" className="extension-error">{error}</div>}
    {!row ? loading && <div className="small muted" role="status">正在读取资源…</div> : <div className="detail-layout">
      <div className="detail-main">
        <section className="card detail-section" aria-label="Agent 配置">
          <h2><Icon name="bot" />Agent 配置</h2>
          <p className="small muted">仅修改用户级配置，按 Agent 原生规则作用于各项目。已有终端请重新加载或重新打开。</p>
          {resourceAgents(locations).map(agent => {
            const control = row.skill_controls.find(c => c.scope === "user" && c.agent === agent);
            const native = userLocations.find(l => l.agent === agent);
            const canToggleNative = editable && !!native && kind !== "skill" && (kind === "package" ? agent !== "pi" : agent !== "claude_code" && !!detail.config);
            const state = kind === "skill" ? control?.enabled : row.enabled;
            return <div className="extension-agent-row" key={agent}>
              <div className="extension-agent-info"><AgentIcon agent={agent} size={20} /><div><strong>{AGENT_LABELS[agent]}</strong>
                <span className="small muted">{kind === "skill" ? control ? state === null ? "用户级未设置，按 Agent 默认规则" : state ? "用户级已启用" : "用户级已禁用" : "由 Agent 加载，暂无独立启停入口"
                  : row.enabled === null ? "启用状态由 Agent 管理" : row.enabled ? "已启用" : "已禁用"}</span></div></div>
              <div className="extension-agent-actions">
                {control && <>
                  <button className="btn small" disabled={busy || loading} onClick={() => void operation(() => api.setExtensionSkillState(USER_SCOPE, row.id, "user", control.revision, control.enabled === false))}>{control.enabled === false ? "启用" : "禁用"}</button>
                  {control.enabled !== null && <button className="btn small ghost" disabled={busy || loading} onClick={() => void operation(() => api.setExtensionSkillState(USER_SCOPE, row.id, "user", control.revision, null))}>恢复默认</button>}
                </>}
                {canToggleNative && <button className="btn small" disabled={busy || loading} onClick={() => {
                  if (kind === "package") packageAction(row.enabled ? "disable" : "enable");
                  else {
                    const config = { ...detail.config, [agent === "antigravity" ? "disabled" : "enabled"]: agent === "antigravity" ? row.enabled : !row.enabled };
                    void operation(() => api.saveExtensionMcp(USER_SCOPE, native!.id, row.revision, row.name, config, false));
                  }
                }}>{row.enabled ? "禁用" : "启用"}</button>}
              </div>
            </div>;
          })}
          {!userLocations.length && <p className="small muted">这是项目级资源，此页仅查看，不修改项目配置。</p>}
          {packages.length > 0 && <div className="extension-package-links"><span className="small muted">随所属插件包启停：</span>{packages.map(parent => <button className="btn small" key={parent.id} onClick={() => navigate(extensionDetailRoute(parent, projectId, workspacePathId))}>{parent.name}<Icon name="chevronRight" /></button>)}</div>}
          {row.managed && !packages.length && <p className="small muted">{source}，由提供方管理。</p>}
        </section>
        <section className="card detail-section" aria-label="资源详情">
          <h2><Icon name={kind === "mcp" ? "sliders" : kind === "skill" ? "spark" : "plugin"} />{EXTENSION_LABELS[kind]}</h2>
          {row.description && <p className="extension-description">{row.description}</p>}
          {row.error && <div className="extension-error">{row.error}</div>}
          {kind === "skill" ? <div className="extension-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children }) => <span>{children}</span>, img: () => null }}>{detail.content.replace(/^\uFEFF?---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")}</ReactMarkdown><details><summary className="small muted">查看 SKILL.md 原文</summary><pre className="extension-source">{detail.content}</pre></details></div>
            : kind === "mcp" ? <pre className="extension-source">{detail.content}</pre>
              : <p className="small muted">插件包由对应 Agent 的原生机制管理；包内资源随插件包安装和移除。</p>}
        </section>
        {kind === "package" && <section className="card detail-section">
          <h2><Icon name="plugin" />包含资源 ({children.length})</h2>
          {!children.length && <p className="small muted">未发现可读取的 Skill 或 MCP；其他能力由 Agent 管理。</p>}
          {children.map(child => <button className="extension-child" key={child.id} onClick={() => navigate(extensionDetailRoute(child, projectId, workspacePathId))}><span><Icon name={child.kind === "skill" ? "spark" : "sliders"} />{child.name}</span><span className="small muted">{EXTENSION_LABELS[child.kind]}<Icon name="chevronRight" /></span></button>)}
        </section>}
      </div>
      <aside className="detail-aside">
        <section className="card detail-section">
          <h2><Icon name="info" />属性</h2>
          <dl className="extension-properties"><dt>来源</dt><dd>{source}</dd><dt>配置范围</dt><dd>{userLocations.length ? "用户级 · 所有项目" : "项目级 · 仅查看"}</dd><dt>支持的 Agent</dt><dd>{resourceAgents(locations).map(a => AGENT_LABELS[a]).join("、") || "由提供方管理"}</dd></dl>
          <span className="extension-path mono">{row.path}</span>
        </section>
        {kind === "skill" && <section className="card detail-section">
          <h2><Icon name="folder" />加载入口 ({row.entry_paths.length})</h2>
          <p className="small muted">内容只读。移除共享源目录会影响所有使用它的 Agent；链接入口只移除链接。</p>
          {row.entry_paths.map((entry, i) => {
            const loc = inventory.locations.find(l => l.id === row.locations[i]);
            return <div className="extension-entry" key={`${loc?.id}:${entry}`}><div><span className="small">{loc?.label}</span><span className="extension-path mono">{entry}</span></div>
              {loc?.scope === "user" && loc.writable && !loc.package && <button className="btn ghost icon-button danger" disabled={busy || loading} aria-label={`移除 ${entry}`} title="移除加载入口" onClick={() => setRemoveEntry(entry)}><Icon name="trash" /></button>}
            </div>;
          })}
        </section>}
      </aside>
    </div>}
    {editing && detail && <ExtensionInstallDialog kind="mcp" scope={USER_SCOPE} scopeName="user" paths={[]} locations={userLocations} agent={userLocation?.agent ?? "codex"} detail={detail} onClose={() => setEditing(false)} onDone={() => { setEditing(false); void refresh().catch(() => {}); }} />}
    {removeEntry !== null && row && <Modal title={kind === "package" ? "卸载插件包" : "移除资源"} onClose={() => { if (!busy) setRemoveEntry(null); }}>
      <p>将移除 <strong>{row.name}</strong> 的用户级{kind === "mcp" ? "配置" : kind === "skill" ? "加载入口" : "插件包"}。</p>
      <p className="small muted">{kind === "skill" ? "普通目录会删除其中全部文件；链接只移除链接，保留源文件。" : kind === "mcp" ? "只删除此 MCP 配置，保留其他配置，不卸载它调用的程序。" : "由对应 Agent 执行原生卸载，包内资源一并受影响。"}</p>
      {removeEntry && <p className="extension-path mono">{removeEntry}</p>}
      {error && <div role="alert" className="extension-error">{error}</div>}
      <div className="row extension-dialog-actions"><button className="btn" disabled={busy} onClick={() => setRemoveEntry(null)}>取消</button><button className="btn danger" disabled={busy} onClick={() => {
        if (kind === "skill") void operation(() => api.removeExtensionSkill(USER_SCOPE, row.id, removeEntry, row.revision));
        else if (kind === "mcp") void operation(() => api.removeExtensionMcp(USER_SCOPE, row.id, row.revision));
        else packageAction("remove");
      }}>{busy ? "处理中…" : "确认移除"}</button></div>
    </Modal>}
  </div>;
}
