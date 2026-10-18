import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import CustomSelect from "../../components/CustomSelect";
import PageHeader from "../../layout/PageHeader";
import { preferenceValues, useViewPreference, useViewScroll, useViewState } from "../../hooks/useViewState";
import type { ExtensionInventory, ExtensionKind, ExtensionLocation, ExtensionResource, Project, WorkspacePath } from "../../types";
import { EXTENSION_LABELS as LABELS, extensionDetailRoute, resourceLocations, under } from "./extensionPresentation";
import McpDefinitionDialog from "./McpDefinitionDialog";
import SkillSourceDialog from "./SkillSourceDialog";
import ExtensionConfigDialog from "./ExtensionConfigDialog";
import McpRegisterDialog from "./McpRegisterDialog";
import SkillRegisterDialog from "./SkillRegisterDialog";
import "./extensions.css";

type ExtensionSort = "name" | "scope";
type RegistrationFilter = "" | "registered" | "unregistered";

const EMPTY: ExtensionInventory = { resources: [], locations: [], warnings: [] };

export default function ExtensionsView({ initialProjectId = "", initialTab = "skill", navigate }: {
  initialProjectId?: string; initialTab?: ExtensionKind; navigate: (route: Route) => void;
}) {
  const stateKey = `extensions.${initialProjectId}:${initialTab}`;
  const [tab, setTab] = useViewState<ExtensionKind>(`${stateKey}.tab`, initialTab);
  const [filter, setFilter] = useViewState(`${stateKey}.project`, initialProjectId);
  const [registration, setRegistration] = useViewState<RegistrationFilter>(`${stateKey}.registration`, "");
  const [query, setQuery] = useViewState(`${stateKey}.query`, "");
  const [viewMode, setViewMode] = useViewPreference<"cards" | "list">("extensions.viewMode", "cards", preferenceValues("cards", "list"));
  const [sort, setSort] = useViewPreference<ExtensionSort>("extensions.sort", "name", preferenceValues("name", "scope"));
  const [projects, setProjects] = useState<Project[]>([]);
  const [paths, setPaths] = useState<WorkspacePath[]>([]);
  const [inventory, setInventory] = useState<ExtensionInventory>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState(false);
  const [defining, setDefining] = useState(false);
  const [configuring, setConfiguring] = useState<ExtensionResource | null>(null);
  const [registering, setRegistering] = useState<ExtensionResource | null>(null);
  const [registeringSkill, setRegisteringSkill] = useState<ExtensionResource | null>(null);
  const request = useRef(0);
  const projectFilter = filter && filter !== "user" ? filter : "";
  const projectEntriesOf = (locations: ExtensionLocation[]) => projectFilter
    ? locations.filter(l => l.scope === "project" && paths.some(p => under(l.path, p.canonical_path)))
    : locations.filter(l => l.scope === "project");

  useEffect(() => { let live = true; void api.listProjects().then(rows => { if (live) setProjects(rows); }).catch(e => { if (live) setError(String(e)); }); return () => { live = false; }; }, []);
  useEffect(() => {
    let live = true;
    setPaths([]);
    if (projectFilter) void api.getProjectDetail(projectFilter).then(detail => {
      if (live) setPaths(detail.workspace_paths);
    }).catch(e => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [projectFilter]);

  const refresh = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent) setLoading(true);
    setError("");
    try {
      const value = await api.listExtensionCatalog();
      if (seq === request.current) setInventory(value);
    } catch (e) {
      if (seq === request.current) { setError(String(e)); setInventory(EMPTY); }
    } finally { if (seq === request.current && !silent) setLoading(false); }
  }, []);
  useEffect(() => { setAdding(false); setDefining(false); setConfiguring(null); setRegistering(null); setRegisteringSkill(null); void refresh(); return () => { ++request.current; }; }, [refresh]);

  const rows = inventory.resources.filter(row => {
    const locations = resourceLocations(inventory, row);
    if (row.kind !== tab) return false;
    if (registration === "registered" && !row.registered) return false;
    if (registration === "unregistered" && row.registered) return false;
    const userEntries = locations.filter(l => l.scope === "user");
    const projectEntries = projectEntriesOf(locations);
    if (filter === "user" && !userEntries.length) return false;
    if (projectFilter && !userEntries.length && !projectEntries.length) return false;
    return `${row.name} ${row.description} ${row.path}`.toLowerCase().includes(query.toLowerCase());
  });
  const scopeRank = (row: ExtensionResource) => {
    const locations = resourceLocations(inventory, row);
    if (locations.some(location => location.scope === "user")) return 0;
    return locations.some(location => location.scope === "project") ? 1 : 2;
  };
  const visibleRows = [...rows].sort(sort === "scope"
    ? (a, b) => scopeRank(a) - scopeRank(b) || a.name.localeCompare(b.name, "zh-Hans")
    : (a, b) => a.name.localeCompare(b.name, "zh-Hans"));
  const scrollRef = useViewScroll(`${stateKey}.scroll`, !loading);

  return <div className="main board-page extensions-page" ref={scrollRef}>
    <PageHeader title="扩展" actions={<>
      <div className="settings-seg icon-seg" role="group" aria-label="扩展类别">
        <button className={tab === "skill" ? "on" : ""} aria-pressed={tab === "skill"} aria-label="Skills" title="Skills" onClick={() => setTab("skill")}><Icon name="skill" /></button>
        <button className={tab === "mcp" ? "on" : ""} aria-pressed={tab === "mcp"} aria-label="MCP" title="MCP" onClick={() => setTab("mcp")}><Icon name="mcp" /></button>
      </div>
      <span className="session-action-divider" aria-hidden="true" />
      {tab === "mcp" && <button className="btn ghost icon-button" aria-label="新增 MCP 定义" title="新增 MCP 定义" disabled={loading} onClick={() => setDefining(true)}><Icon name="plus" /></button>}
      {tab === "skill" && <button className="btn ghost icon-button" aria-label="托管 Skill 源" title="托管 Skill 源" disabled={loading} onClick={() => setAdding(true)}><Icon name="plus" /></button>}
    </>} />
    <div className="board-toolbar">
      <input type="text" className="ws-search" aria-label="搜索扩展" placeholder="搜索扩展…（名称、说明或路径）" value={query} onChange={e => setQuery(e.target.value)} />
      <div className="toolbar ws-controls">
        <div className="ws-controls-group">
          <div className="ws-control"><span className="muted small">范围</span>
            <CustomSelect value={filter} onChange={setFilter} ariaLabel="筛选范围" options={[
              { value: "", label: "全部扩展" },
              { value: "user", label: "用户级扩展" },
              ...projects.map(p => ({ value: p.id, label: `${p.name}${p.visibility === "archived" ? "（已归档）" : ""}` })),
            ]} />
          </div>
          <div className="ws-control"><span className="muted small">托管</span>
            <CustomSelect value={registration} onChange={value => setRegistration(value as RegistrationFilter)} ariaLabel="筛选托管状态" options={[{ value: "", label: "全部" }, { value: "registered", label: "已托管" }, { value: "unregistered", label: "未托管" }]} />
          </div>
          <div className="ws-control"><span className="muted small">排序</span>
            <CustomSelect value={sort} onChange={value => setSort(value as ExtensionSort)} ariaLabel="排序" options={[{ value: "name", label: "名称" }, { value: "scope", label: "启用范围" }]} />
          </div>
          {(query || filter || registration) && <button className="btn small ghost" onClick={() => { setQuery(""); setFilter(""); setRegistration(""); }}>清除筛选</button>}
        </div>
        <div className="ws-controls-meta">
          <span className="muted small">{query || filter || registration
            ? `显示 ${rows.length} / 共 ${inventory.resources.filter(row => row.kind === tab).length} 项`
            : `共 ${rows.length} 项`}</span>
          <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
            <button className={viewMode === "cards" ? "on" : ""} aria-pressed={viewMode === "cards"} aria-label="卡片视图" title="卡片视图" onClick={() => setViewMode("cards")}><Icon name="grid" /></button>
            <button className={viewMode === "list" ? "on" : ""} aria-pressed={viewMode === "list"} aria-label="列表视图" title="列表视图" onClick={() => setViewMode("list")}><Icon name="list" /></button>
          </div>
        </div>
      </div>
    </div>
    {error && <div role="alert" className="extension-error">{error}</div>}
    {inventory.warnings.length > 0 && <details className="extension-warnings"><summary>部分来源无法读取（{inventory.warnings.length}）</summary>{inventory.warnings.map((warning, i) => <p key={i}>{warning}</p>)}</details>}
    {loading ? <div className="muted small" role="status">正在读取本地资源…</div> : <section className={viewMode === "cards" ? "ws-grid board-grid extension-cards" : "extension-list"} aria-label={LABELS[tab]}>
      {!visibleRows.length && <div className="empty">没有匹配的{LABELS[tab]}。</div>}{visibleRows.map(row => {
        const locations = resourceLocations(inventory, row);
        const userEntries = locations.filter(l => l.scope === "user");
        const enabled = userEntries.length > 0 || projectEntriesOf(locations).length > 0;
        const canConfigure = row.registered;
        return <div key={row.id} role="button" tabIndex={0} className={`extension-row extension-${viewMode}`}
          onClick={() => navigate(extensionDetailRoute(row))}
          onKeyDown={e => {
            if (e.target !== e.currentTarget) return;
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              navigate(extensionDetailRoute(row));
            }
          }}>
          <span className="extension-row-heading">
            <Icon name={row.kind === "mcp" ? "mcp" : "skill"} /><strong>{row.name}</strong>
            <span className="extension-row-badges">{row.error && <span className="badge warn">配置异常</span>}{!enabled && <span className="badge">未启用</span>}</span>
          </span>
          {row.description && <span className="extension-description">{row.description}</span>}
          <span className="extension-row-actions">
            {canConfigure
              ? <button className="btn ghost icon-button small" aria-label="配置" title="配置" onClick={e => { e.stopPropagation(); setConfiguring(row); }}><Icon name="sliders" /></button>
              : row.kind === "skill"
                ? <button className="btn ghost icon-button small" aria-label="托管" title="托管到 NoEnding 后才能在用户级和项目中配置" onClick={e => { e.stopPropagation(); setRegisteringSkill(row); }}><Icon name="adopt" /></button>
                : <button className="btn ghost icon-button small" aria-label="托管" title="将该原生配置托管到 NoEnding，之后可在用户级或项目中启停" onClick={e => { e.stopPropagation(); setRegistering(row); }}><Icon name="adopt" /></button>}
          </span>
        </div>;
      })}
    </section>}
    {adding && <SkillSourceDialog onClose={() => setAdding(false)} onDone={() => { setAdding(false); void refresh(); }} />}
    {defining && <McpDefinitionDialog onClose={() => setDefining(false)} onDone={() => { setDefining(false); void refresh(); }} />}
    {configuring && <ExtensionConfigDialog resource={configuring} locations={inventory.locations}
      onClose={() => setConfiguring(null)} onDone={async () => { await refresh(true); setConfiguring(null); }} />}
    {registering && <McpRegisterDialog row={registering}
      onClose={() => setRegistering(null)} onDone={() => { setRegistering(null); void refresh(true); }} />}
    {registeringSkill && <SkillRegisterDialog id={registeringSkill.id} name={registeringSkill.name}
      onClose={() => setRegisteringSkill(null)} onDone={() => { setRegisteringSkill(null); void refresh(true); }} />}
  </div>;
}
