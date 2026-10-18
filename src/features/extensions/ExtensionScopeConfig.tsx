import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import type { Route } from "../../app/routes";
import type { ExtensionInventory, ExtensionLocation, ExtensionResource } from "../../types";
import { EXTENSION_AGENTS } from "./extensionAgents";
import { extensionDetailRoute, locationSupportsAgent } from "./extensionPresentation";
import ProjectExtensionDialog from "./ProjectExtensionDialog";
import "./extensions.css";

const EMPTY: ExtensionInventory = { resources: [], locations: [], warnings: [] };

type ScopedEntry = { path: string; link: boolean; location: ExtensionLocation };

function entriesIn(inventory: ExtensionInventory, row: ExtensionResource, scope: "user" | "project"): ScopedEntry[] {
  return row.entry_paths.flatMap((path, index) => {
    const location = inventory.locations.find(item => item.id === row.locations[index]);
    if (!location || location.scope !== scope) return [];
    return [{ path, link: row.kind === "mcp" || row.entry_links[index] === true, location }];
  });
}

function inside(path: string, root: string) {
  const base = root.replace(/[\\/]+$/, "");
  return path === base || path.startsWith(`${base}/`) || path.startsWith(`${base}\\`);
}

/** Project configuration for Skill sources and MCP definitions. */
export default function ExtensionScopeConfig({ projectId, workspacePaths = [], navigate }: {
  projectId: string; workspacePaths?: string[]; navigate: (route: Route) => void;
}) {
  const [inventory, setInventory] = useState<ExtensionInventory>(EMPTY);
  const [catalog, setCatalog] = useState<ExtensionInventory>(EMPTY);
  const [userInventory, setUserInventory] = useState<ExtensionInventory | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState("");
  const request = useRef(0);
  const alive = useRef(true);

  const load = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent) setLoading(true);
    setError("");
    try {
      const [scopeInventory, user, all] = await Promise.all([
        api.listExtensionResources({ project_id: projectId }),
        api.listExtensionResources({}),
        api.listExtensionCatalog(),
      ]);
      if (seq !== request.current || !alive.current) return;
      setInventory(scopeInventory);
      setUserInventory(user);
      setCatalog(all);
    } catch (e) {
      if (seq === request.current && alive.current) { setInventory(EMPTY); setUserInventory(null); setCatalog(EMPTY); setError(String(e)); }
    } finally {
      if (seq === request.current && alive.current && !silent) setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    alive.current = true;
    setAdding(false);
    void load();
    return () => { alive.current = false; ++request.current; };
  }, [load]);

  const rows = useMemo(() => catalog.resources.filter(row => row.kind === "skill" || (row.kind === "mcp" && row.registered)), [catalog]);
  const projectEntries = (row: ExtensionResource) => entriesIn(inventory, row, "project");
  const userEntries = (row: ExtensionResource) => userInventory ? entriesIn(userInventory, row, "user") : [];
  const visibleRows = rows.filter(row => projectEntries(row).length > 0 || userEntries(row).length > 0);
  const enabledIds = (pick: (row: ExtensionResource) => ScopedEntry[]) =>
    rows.filter(row => pick(row).length > 0).map(row => row.id);

  const apply = async (row: ExtensionResource, enable: boolean) => {
    setBusy(row.id); setError("");
    try {
      const result = row.kind === "mcp"
        ? enable ? await api.enableMcpDefinitionProjects([projectId], row.id) : await api.disableMcpDefinitionProjects([projectId], row.id)
        : enable ? await api.enableExtensionSkillProjects([projectId], row.id) : await api.disableExtensionSkillProjects([projectId], row.id);
      showToast(result.message);
      await load(true);
    } catch (e) {
      if (alive.current) setError(String(e));
    } finally {
      if (alive.current) setBusy("");
    }
  };

  return <section className="card detail-section" aria-label="扩展">
    <div className="row between" style={{ alignItems: "center", minHeight: 24 }}>
      <h2 className="detail-section-title">
        <Icon name="extensions" />
        <span>扩展 ({visibleRows.length})</span>
      </h2>
      <button type="button" className="btn ghost icon-button small" title="配置扩展" aria-label="配置扩展" disabled={loading} onClick={() => setAdding(true)}>
        <Icon name="sliders" />
      </button>
    </div>
    {error && <div role="alert" className="extension-error">{error}</div>}
    {loading ? <p className="small muted" role="status">正在读取扩展配置…</p>
      : !visibleRows.length ? <p className="small muted">当前项目还没有启用的扩展。</p>
      : <div className="extension-config-list">
        {visibleRows.map(row => {
          const entries = projectEntries(row);
          const userScopeEntries = userEntries(row);
          const enabled = entries.length > 0;
          const physical = entries.some(entry => !entry.link);
          const full = workspacePaths.length > 0 && workspacePaths.every(path => {
            const inPath = entries.filter(entry => inside(entry.path, path));
            return EXTENSION_AGENTS.every(agent => inPath.some(entry => row.kind === "mcp" ? entry.location.agent === agent : locationSupportsAgent(entry.location, agent)));
          });
          const userEffective = userScopeEntries.length > 0;
          return <div className="extension-config-row" key={row.id} role="button" tabIndex={0}
            onClick={() => navigate(extensionDetailRoute(row))}
            onKeyDown={e => {
              if (e.target !== e.currentTarget) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                navigate(extensionDetailRoute(row));
              }
            }}>
            <div className="extension-config-copy">
              <span className="extension-config-name"><Icon name={row.kind === "mcp" ? "mcp" : "skill"} />{row.name}</span>
              {row.error && <span className="small extension-config-error">{row.error}</span>}
            </div>
            <div className="extension-config-actions">
              {!row.registered && <span className="badge">未托管</span>}
              {physical && <span className="badge" title="该加载入口是真实目录而不是链接；NoEnding 不停用该入口，迁移或卸载时才会处理">物理入口</span>}
              {!userEffective && row.registered && enabled && !full && <button className="btn small" disabled={busy === row.id} onClick={e => { e.stopPropagation(); void apply(row, true); }}>补齐</button>}
            </div>
          </div>;
        })}
      </div>}
    {adding && <ProjectExtensionDialog projectId={projectId} resources={rows.filter(row => row.registered)}
      enabledIds={enabledIds(projectEntries)} userEffectiveIds={enabledIds(userEntries)}
      onClose={() => setAdding(false)} onDone={async () => { await load(true); setAdding(false); }} />}
  </section>;
}
