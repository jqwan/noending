import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import CustomSelect from "../../components/CustomSelect";
import PageHeader from "../../layout/PageHeader";
import { preferenceValues, useViewPreference, useViewScroll, useViewState } from "../../hooks/useViewState";
import type { SkillInventory, SkillLocation, SkillResource, Project, WorkspacePath } from "../../types";
import { displayScanWarning, skillDetailRoute, resourceLocations, under } from "./skillPresentation";
import SkillSourceDialog from "./SkillSourceDialog";
import SkillConfigDialog from "./SkillConfigDialog";
import SkillRegisterDialog from "./SkillRegisterDialog";
import { skillDetailCache, skillCatalogCache, setSkillCatalogCache } from "./skillDetailCache";
import "./skills.css";

type SkillSort = "name" | "scope";
type RegistrationFilter = "registered" | "unregistered";

const EMPTY: SkillInventory = { resources: [], locations: [], warnings: [] };

export default function SkillsView({ initialProjectId = "", navigate }: {
  initialProjectId?: string; navigate: (route: Route) => void;
}) {
  const stateKey = `skills.${initialProjectId}`;
  const [filter, setFilter] = useViewState(`${stateKey}.project`, initialProjectId);
  const [registration, setRegistration] = useViewState<RegistrationFilter>(`${stateKey}.registration`, "registered");
  const [query, setQuery] = useViewState(`${stateKey}.query`, "");
  const [viewMode, setViewMode] = useViewPreference<"cards" | "list">("skills.viewMode", "cards", preferenceValues("cards", "list"));
  const [sort, setSort] = useViewPreference<SkillSort>("skills.sort", "name", preferenceValues("name", "scope"));
  const [projects, setProjects] = useState<Project[]>([]);
  const [paths, setPaths] = useState<WorkspacePath[]>([]);
  const [inventory, setInventory] = useState<SkillInventory>(() => skillCatalogCache ?? EMPTY);
  const [loading, setLoading] = useState(() => !skillCatalogCache);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState(false);
  const [configuring, setConfiguring] = useState<SkillResource | null>(null);
  const [registeringSkill, setRegisteringSkill] = useState<SkillResource | null>(null);
  const request = useRef(0);
  const projectFilter = filter && filter !== "user" ? filter : "";
  const projectEntriesOf = (locations: SkillLocation[]) => projectFilter
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
    if (!silent && !skillCatalogCache) setLoading(true);
    setError("");
    try {
      const value = await api.listSkillCatalog();
      if (seq === request.current) {
        setSkillCatalogCache(value);
        setInventory(value);
      }
    } catch (e) {
      if (seq === request.current) {
        setError(String(e));
        if (!skillCatalogCache) setInventory(EMPTY);
      }
    } finally {
      if (seq === request.current && !silent) setLoading(false);
    }
  }, []);
  useEffect(() => {
    setAdding(false);
    setConfiguring(null);
    setRegisteringSkill(null);
    void refresh(!!skillCatalogCache);
    return () => { ++request.current; };
  }, [refresh]);

  const categoryRows = inventory.resources.filter(row => row.registered === (registration === "registered"));
  const rows = categoryRows.filter(row => {
    const locations = resourceLocations(inventory, row);
    const userEntries = locations.filter(l => l.scope === "user");
    const projectEntries = projectEntriesOf(locations);
    if (filter === "user" && !userEntries.length) return false;
    if (projectFilter && !userEntries.length && !projectEntries.length) return false;
    return `${row.name} ${row.description} ${row.path}`.toLowerCase().includes(query.toLowerCase());
  });
  const scopeRank = (row: SkillResource) => {
    const locations = resourceLocations(inventory, row);
    if (locations.some(location => location.scope === "user")) return 0;
    return locations.some(location => location.scope === "project") ? 1 : 2;
  };
  const visibleRows = [...rows].sort(sort === "scope"
    ? (a, b) => scopeRank(a) - scopeRank(b) || a.name.localeCompare(b.name, "zh-Hans")
    : (a, b) => a.name.localeCompare(b.name, "zh-Hans"));
  const scrollRef = useViewScroll(`${stateKey}.scroll`, !loading);

  return <div className="main board-page skills-page" ref={scrollRef}>
    <PageHeader title="技能" actions={<>
      <div className="settings-seg icon-seg" role="group" aria-label="技能托管状态">
        <button className={registration === "registered" ? "on" : ""} aria-pressed={registration === "registered"} aria-label="已托管" title="已托管" onClick={() => setRegistration("registered")}><Icon name="skill" /></button>
        <button className={registration === "unregistered" ? "on" : ""} aria-pressed={registration === "unregistered"} aria-label="未托管" title="未托管" onClick={() => setRegistration("unregistered")}><Icon name="archive" /></button>
      </div>
      <span className="session-action-divider" aria-hidden="true" />
      <button className="btn ghost icon-button" aria-label="托管 Skill 源" title="托管 Skill 源" disabled={loading} onClick={() => setAdding(true)}><Icon name="plus" /></button>
    </>} />
    <div className="board-toolbar">
      <input type="text" className="ws-search" aria-label="搜索技能" placeholder="搜索技能…（名称、说明或路径）" value={query} onChange={e => setQuery(e.target.value)} />
      <div className="toolbar ws-controls">
        <div className="ws-controls-group">
          <div className="ws-control"><span className="muted small">范围</span>
            <CustomSelect value={filter} onChange={setFilter} ariaLabel="筛选范围" options={[
              { value: "", label: "全部技能" },
              { value: "user", label: "用户级技能" },
              ...projects.map(p => ({ value: p.id, label: `${p.name}${p.visibility === "archived" ? "（已归档）" : ""}` })),
            ]} />
          </div>
          <div className="ws-control"><span className="muted small">排序</span>
            <CustomSelect value={sort} onChange={value => setSort(value as SkillSort)} ariaLabel="排序" options={[{ value: "name", label: "名称" }, { value: "scope", label: "启用范围" }]} />
          </div>
          {(query || filter) && <button className="btn small ghost" onClick={() => { setQuery(""); setFilter(""); }}>清除筛选</button>}
        </div>
        <div className="ws-controls-meta">
          <span className="muted small">{query || filter
            ? `显示 ${rows.length} / 共 ${categoryRows.length} 项`
            : `共 ${rows.length} 项`}</span>
          <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
            <button className={viewMode === "cards" ? "on" : ""} aria-pressed={viewMode === "cards"} aria-label="卡片视图" title="卡片视图" onClick={() => setViewMode("cards")}><Icon name="grid" /></button>
            <button className={viewMode === "list" ? "on" : ""} aria-pressed={viewMode === "list"} aria-label="列表视图" title="列表视图" onClick={() => setViewMode("list")}><Icon name="list" /></button>
          </div>
        </div>
      </div>
    </div>
    {error && <div role="alert" className="skill-error">{error}</div>}
    {inventory.warnings.length > 0 && <details className="skill-warnings"><summary>部分来源无法读取（{inventory.warnings.length}）</summary>{inventory.warnings.map((warning, i) => <p key={i}>{displayScanWarning(warning)}</p>)}</details>}
    <section className={viewMode === "cards" ? "ws-grid board-grid skill-cards" : "skill-list"} aria-label="技能">
      {!loading && !visibleRows.length && <div className="empty">没有匹配的技能。</div>}
      {visibleRows.map(row => {
        const locations = resourceLocations(inventory, row);
        const userEntries = locations.filter(l => l.scope === "user");
        const enabled = userEntries.length > 0 || projectEntriesOf(locations).length > 0;
        const canConfigure = row.registered;

        if (viewMode === "cards") {
          return (
            <article
              key={`card-${row.id}`}
              className="ws-card full skill-card"
              tabIndex={0}
              role="button"
              onClick={() => navigate(skillDetailRoute(row))}
              onPointerEnter={() => {
                if (!skillDetailCache.has(row.id)) {
                  api.getSkillDetail(row.id).then(bundle => {
                    if (bundle) skillDetailCache.set(row.id, bundle);
                  }).catch(() => {});
                }
              }}
              onKeyDown={e => {
                if (e.target !== e.currentTarget) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  navigate(skillDetailRoute(row));
                }
              }}
            >
              <header className="ws-card-head">
                <h3 className="ws-card-title" title={row.name}>
                  <Icon name="skill" />
                  <span className="truncate">{row.name}</span>
                </h3>
                <div className="ws-card-side">
                  {row.error && <span className="badge warn">配置异常</span>}
                  {!enabled && <span className="badge">未启用</span>}
                </div>
              </header>

              <div className="ws-card-body">
                {row.description && (
                  <p className="ws-card-desc skill-description" title={row.description}>
                    {row.description}
                  </p>
                )}
              </div>

              <footer className="ws-card-meta">
                <span className="muted small">
                  {row.registered ? "已托管" : "本地发现"}
                </span>
                <div className="ws-card-actions" onClick={e => e.stopPropagation()}>
                  {canConfigure ? (
                    <button
                      type="button"
                      className="btn small ghost icon-button"
                      aria-label="配置"
                      title="配置"
                      onClick={e => {
                        e.stopPropagation();
                        setConfiguring(row);
                      }}
                    >
                      <Icon name="sliders" />
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="btn small ghost icon-button"
                      aria-label="托管"
                      title="托管到 NoEnding 后才能在用户级和项目中配置"
                      onClick={e => {
                        e.stopPropagation();
                        setRegisteringSkill(row);
                      }}
                    >
                      <Icon name="adopt" />
                    </button>
                  )}
                </div>
              </footer>
            </article>
          );
        }

        return (
          <article
            key={`list-${row.id}`}
            className="skill-list-row"
          >
            <button
              type="button"
              className="skill-open"
              onClick={() => navigate(skillDetailRoute(row))}
              onPointerEnter={() => {
                if (!skillDetailCache.has(row.id)) {
                  api.getSkillDetail(row.id).then(bundle => {
                    if (bundle) skillDetailCache.set(row.id, bundle);
                  }).catch(() => {});
                }
              }}
            >
              <div className="skill-list-title" title={row.name}>
                <Icon name="skill" />
                <span>{row.name}</span>
                {row.error && <span className="badge warn">配置异常</span>}
                {!enabled && <span className="badge">未启用</span>}
              </div>
              <div className="skill-list-meta">
                <span>{row.registered ? "已托管" : "本地发现"}</span>
                {row.description && (
                  <>
                    <span>·</span>
                    <span className="skill-list-desc truncate" title={row.description}>
                      {row.description}
                    </span>
                  </>
                )}
              </div>
            </button>
            <div className="skill-list-actions" onClick={e => e.stopPropagation()}>
              {canConfigure ? (
                <button
                  type="button"
                  className="btn small ghost icon-button"
                  aria-label="配置"
                  title="配置"
                  onClick={e => {
                    e.stopPropagation();
                    setConfiguring(row);
                  }}
                >
                  <Icon name="sliders" />
                </button>
              ) : (
                <button
                  type="button"
                  className="btn small ghost icon-button"
                  aria-label="托管"
                  title="托管到 NoEnding 后才能在用户级和项目中配置"
                  onClick={e => {
                    e.stopPropagation();
                    setRegisteringSkill(row);
                  }}
                >
                  <Icon name="adopt" />
                </button>
              )}
            </div>
          </article>
        );
      })}
    </section>
    {adding && <SkillSourceDialog onClose={() => setAdding(false)} onDone={() => { setAdding(false); void refresh(); }} />}
    {configuring && <SkillConfigDialog resource={configuring} locations={inventory.locations}
      onClose={() => setConfiguring(null)} onDone={async () => { await refresh(true); setConfiguring(null); }} />}
    {registeringSkill && <SkillRegisterDialog id={registeringSkill.id} name={registeringSkill.name}
      onClose={() => setRegisteringSkill(null)} onDone={() => { skillDetailCache.delete(registeringSkill.id); setRegisteringSkill(null); void refresh(true); }} />}
  </div>;
}
