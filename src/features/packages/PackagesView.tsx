import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import CustomSelect from "../../components/CustomSelect";
import { Modal } from "../../components/common";
import PageHeader from "../../layout/PageHeader";
import { showToast } from "../../components/Toast";
import { preferenceValues, useViewPreference, useViewScroll, useViewState } from "../../hooks/useViewState";
import { AGENT_LABELS, type PackageEntry, type PackageInventory } from "../../types";
import { entryState, originClassLabel, originLabel, packageDetailRoute, shortVersion } from "./packagePresentation";
import PackageImportDialog from "./PackageImportDialog";
import { packageCatalogCache, packageDetailCache, setPackageCatalogCache } from "./packageDetailCache";
import "./packages.css";

const EMPTY: PackageInventory = { packages: [], warnings: [] };

/** 可管理 = NoEnding can act on the package at all: toggle its switch and/or
 *  uninstall it through the Agent's own machinery. Enablement is a separate
 *  dimension shown as a badge. NoEnding built-ins follow the same rules —
 *  with neither capability they are simply unmanageable. */
type PackageTab = "managed" | "unmanaged";

export default function PackagesView({ navigate }: { navigate: (route: Route) => void }) {
  const [tab, setTab] = useViewState<PackageTab>("packages.tab", "managed");
  const [agentFilter, setAgentFilter] = useViewState("packages.agent", "");
  const [query, setQuery] = useViewState("packages.query", "");
  const [viewMode, setViewMode] = useViewPreference<"cards" | "list">("packages.viewMode", "cards", preferenceValues("cards", "list"));
  const [inventory, setInventory] = useState<PackageInventory>(() => packageCatalogCache ?? EMPTY);
  const [loading, setLoading] = useState(() => !packageCatalogCache);
  const [error, setError] = useState("");
  const [importing, setImporting] = useState(false);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [uninstalling, setUninstalling] = useState<PackageEntry | null>(null);
  const [uninstallBusy, setUninstallBusy] = useState(false);
  const request = useRef(0);

  const refresh = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent && !packageCatalogCache) setLoading(true);
    setError("");
    try {
      const value = await api.listPackages();
      if (seq === request.current) {
        setPackageCatalogCache(value);
        setInventory(value);
      }
    } catch (e) {
      if (seq === request.current) {
        setError(String(e));
        if (!packageCatalogCache) setInventory(EMPTY);
      }
    } finally {
      if (seq === request.current && !silent) setLoading(false);
    }
  }, []);
  useEffect(() => {
    setImporting(false);
    void refresh(!!packageCatalogCache);
    return () => { ++request.current; };
  }, [refresh]);

  const agents = [...new Set(inventory.packages.flatMap(entry => entry.agent ? [entry.agent] : []))];
  const isManaged = (entry: PackageEntry) =>
    entry.capabilities.toggle || entry.capabilities.uninstall;
  const categoryRows = inventory.packages.filter(entry => tab === "managed" ? isManaged(entry) : !isManaged(entry));
  const rows = categoryRows.filter(entry => {
    if (agentFilter && entry.agent !== agentFilter) return false;
    return `${entry.name} ${entry.description} ${entry.marketplace ?? ""}`.toLowerCase().includes(query.toLowerCase());
  });
  const visibleRows = [...rows].sort((a, b) => a.name.localeCompare(b.name, "zh-Hans"));
  const scrollRef = useViewScroll("packages.scroll", !loading);

  const openDetail = (entry: PackageEntry) => navigate(packageDetailRoute(entry));
  const prefetch = (entry: PackageEntry) => {
    if (!packageDetailCache.has(entry.id)) {
      api.getPackageDetail(entry.id).then(detail => {
        if (detail) packageDetailCache.set(entry.id, detail);
      }).catch(() => {});
    }
  };

  // Only entries with a user scope and a native toggle get the interactive
  // power switch; the uninstall shortcut rides along whenever the Agent
  // provides an uninstall command.
  const configurable = (entry: PackageEntry) =>
    entry.capabilities.toggle && entry.scopes.some(scope => scope.scope === "user");

  const confirmUninstall = async () => {
    if (!uninstalling || uninstallBusy) return;
    setUninstallBusy(true);
    try {
      const result = await api.uninstallPackage(uninstalling.id);
      showToast(result.message);
      setPackageCatalogCache(null);
      packageDetailCache.delete(uninstalling.id);
      setUninstalling(null);
      await refresh();
    } catch (e) {
      setError(String(e));
      setUninstalling(null);
    } finally {
      setUninstallBusy(false);
    }
  };

  const togglePackage = async (entry: PackageEntry) => {
    if (togglingId) return;
    // Anything the config has not explicitly enabled (disabled or never
    // written) counts as off: gray button, click enables.
    const enable = entryState(entry) !== "enabled";
    setTogglingId(entry.id);
    try {
      const result = enable
        ? await api.enablePackage(entry.id, entry.revision)
        : await api.disablePackage(entry.id, entry.revision);
      showToast(result.message);
      setPackageCatalogCache(null);
      packageDetailCache.delete(entry.id);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setTogglingId(null);
    }
  };

  // Enablement is its own filled badge: 启用 (accent, matching the power
  // switch) / 停用 (explicitly off or never written) / 未知 (the Agent
  // exposes no evidence — read-only boards).
  const stateBadge = (entry: PackageEntry) => {
    const state = entryState(entry);
    if (state === "enabled") return <span className="badge accent">启用</span>;
    if (state === "unknown") return <span className="badge">未知</span>;
    return <span className="badge">停用</span>;
  };

  const cardMeta = (entry: PackageEntry) => {
    const parts = [entry.origin === "profile" ? originLabel(entry) : originClassLabel(entry)];
    return parts.filter(Boolean).join(" · ");
  };
  const cardMetaTitle = (entry: PackageEntry) => {
    const parts = [originClassLabel(entry)];
    if (entry.marketplace) parts.push(entry.marketplace);
    return parts.filter(Boolean).join(" · ");
  };

  // The owning Agent is the most useful per-card identity, so it takes the
  // title icon slot; agentless imports fall back to the resource icon.
  const titleIcon = (entry: PackageEntry) => entry.agent
    ? <span role="img" aria-label={AGENT_LABELS[entry.agent]} title={AGENT_LABELS[entry.agent]}><AgentIcon agent={entry.agent} size={16} /></span>
    : <Icon name="puzzle" />;

  return <div className="main board-page packages-page" ref={scrollRef}>
    <PageHeader title="插件包" actions={<>
      <div className="settings-seg icon-seg" role="group" aria-label="插件包管理状态">
        <button className={tab === "managed" ? "on" : ""} aria-pressed={tab === "managed"} aria-label="可管理" title="可管理（可启停或可卸载）" onClick={() => setTab("managed")}><Icon name="puzzle" /></button>
        <button className={tab === "unmanaged" ? "on" : ""} aria-pressed={tab === "unmanaged"} aria-label="不可管理" title="不可管理（仅查看）" onClick={() => setTab("unmanaged")}><Icon name="archive" /></button>
      </div>
      <span className="session-action-divider" aria-hidden="true" />
      <button className="btn ghost icon-button" aria-label="导入插件包" title="导入本地插件包目录" disabled={loading} onClick={() => setImporting(true)}><Icon name="plus" /></button>
    </>} />
    <div className="board-toolbar">
      <input type="text" className="ws-search" aria-label="搜索插件包" placeholder="搜索插件包…（名称、说明或市场）" value={query} onChange={e => setQuery(e.target.value)} />
      <div className="toolbar ws-controls">
        <div className="ws-controls-group">
          <div className="ws-control"><span className="muted small">Agent</span>
            <CustomSelect value={agentFilter} onChange={setAgentFilter} ariaLabel="筛选 Agent" options={[
              { value: "", label: "全部 Agent" },
              ...agents.map(agent => ({ value: agent, label: AGENT_LABELS[agent] })),
            ]} />
          </div>
          {(query || agentFilter) && <button className="btn small ghost" onClick={() => { setQuery(""); setAgentFilter(""); }}>清除筛选</button>}
        </div>
        <div className="ws-controls-meta">
          <span className="muted small">{query || agentFilter
            ? `显示 ${rows.length} / 共 ${categoryRows.length} 项`
            : `共 ${rows.length} 项`}</span>
          <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
            <button className={viewMode === "cards" ? "on" : ""} aria-pressed={viewMode === "cards"} aria-label="卡片视图" title="卡片视图" onClick={() => setViewMode("cards")}><Icon name="grid" /></button>
            <button className={viewMode === "list" ? "on" : ""} aria-pressed={viewMode === "list"} aria-label="列表视图" title="列表视图" onClick={() => setViewMode("list")}><Icon name="list" /></button>
          </div>
        </div>
      </div>
    </div>
    {error && <div role="alert" className="package-error">{error}</div>}
    {inventory.warnings.length > 0 && <details className="package-warnings"><summary>部分来源无法读取（{inventory.warnings.length}）</summary>{inventory.warnings.map((warning, i) => <p key={i}>{warning}</p>)}</details>}
    {/* 列表与卡片是互斥结构：容器与条目都用视图专属 key，切换时整体卸载重挂，
        避免 React 原地复用 DOM 只改 className 造成卡片轮廓闪一帧（会话/Agent
        看板同一问题的既定修法）。 */}
    <section key={viewMode === "cards" ? "packages-cards" : "packages-list"}
      className={viewMode === "cards" ? "ws-grid board-grid package-cards" : "package-list"} aria-label="插件包">
      {!loading && !visibleRows.length && <div className="empty">{tab === "managed" ? "没有可管理的插件包。" : "没有不可管理的插件包。"}</div>}
      {visibleRows.map(entry => viewMode === "cards" ? (
        <article key={`card-${entry.id}`} className="ws-card full package-card" tabIndex={0} role="button"
          onClick={() => openDetail(entry)}
          onPointerEnter={() => prefetch(entry)}
          onKeyDown={e => {
            if (e.target !== e.currentTarget) return;
            if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openDetail(entry); }
          }}>
          <header className="ws-card-head">
            <h3 className="ws-card-title" title={entry.name}>
              {titleIcon(entry)}
              <span className="truncate">{entry.name}</span>
            </h3>
            <div className="ws-card-side">{stateBadge(entry)}</div>
          </header>
          <div className="ws-card-body">
            {entry.version && <div className="package-card-version" title={entry.version}><span className="package-version-text">v{shortVersion(entry.version)}</span></div>}
            {entry.description && <p className="ws-card-desc" title={entry.description}>{entry.description}</p>}
          </div>
          <footer className="ws-card-meta">
            <span className="muted small package-card-meta" title={cardMetaTitle(entry)}>{cardMeta(entry)}</span>
            {(configurable(entry) || entry.capabilities.uninstall) && <div className="ws-card-actions" onClick={e => e.stopPropagation()}>
              {entry.capabilities.uninstall && <button type="button" className="btn small ghost icon-button danger"
                aria-label="卸载插件包" title="卸载插件包"
                disabled={uninstallBusy}
                onClick={() => setUninstalling(entry)}><Icon name="trash" /></button>}
              {configurable(entry) && <button type="button" className={"btn small ghost icon-button" + (entryState(entry) === "enabled" ? " is-on" : " is-off")}
                aria-label={entryState(entry) === "enabled" ? "停用" : "启用"}
                title={entryState(entry) === "enabled" ? "用户级停用" : "用户级启用"}
                disabled={togglingId === entry.id}
                onClick={() => void togglePackage(entry)}><Icon name="power" /></button>}
            </div>}
          </footer>
        </article>
      ) : (
        <article key={`list-${entry.id}`} className="package-list-row">
          <button type="button" className="package-open" onClick={() => openDetail(entry)} onPointerEnter={() => prefetch(entry)}>
            <div className="package-list-title" title={entry.name}>
              {titleIcon(entry)}
              <span>{entry.name}</span>
              {entry.version && <span className="package-version-text package-version" title={entry.version}>v{shortVersion(entry.version)}</span>}
              {stateBadge(entry)}
            </div>
            <div className="package-list-meta">
              <span>{cardMeta(entry)}</span>
              {entry.description && <span className="package-list-desc" title={entry.description}>{entry.description}</span>}
            </div>
          </button>
          {(configurable(entry) || entry.capabilities.uninstall) && <div className="package-list-actions">
            {entry.capabilities.uninstall && <button type="button" className="btn small ghost icon-button danger"
              aria-label="卸载插件包" title="卸载插件包"
              disabled={uninstallBusy}
              onClick={() => setUninstalling(entry)}><Icon name="trash" /></button>}
            {configurable(entry) && <button type="button" className={"btn small ghost icon-button" + (entryState(entry) === "enabled" ? " is-on" : " is-off")}
              aria-label={entryState(entry) === "enabled" ? "停用" : "启用"}
              title={entryState(entry) === "enabled" ? "用户级停用" : "用户级启用"}
              disabled={togglingId === entry.id}
              onClick={() => void togglePackage(entry)}><Icon name="power" /></button>}
          </div>}
        </article>
      ))}
    </section>
    {uninstalling && <Modal title="卸载插件包" onClose={() => { if (!uninstallBusy) setUninstalling(null); }}>
      <p>将通过 {uninstalling.agent ? AGENT_LABELS[uninstalling.agent] : "该 Agent"} 的原生命令卸载「{uninstalling.name}」。</p>
      <p className="small muted">注册表中的安装条目随卸载一起消失，作用域内的启停配置不再生效；已有终端需要重新加载。源目录不受影响。</p>
      <div className="row package-dialog-actions">
        <button className="btn" disabled={uninstallBusy} onClick={() => setUninstalling(null)}>取消</button>
        <button className="btn danger" disabled={uninstallBusy} onClick={() => void confirmUninstall()}>{uninstallBusy ? "处理中…" : "卸载"}</button>
      </div>
    </Modal>}
    {importing && <PackageImportDialog
      onClose={() => setImporting(false)}
      onDone={message => {
        setImporting(false);
        setPackageCatalogCache(null);
        showToast(message);
        // Fresh entries come from each Agent's own registry after install;
        // stay on the refreshed board instead of guessing the new entry id.
        void refresh();
      }} />}
  </div>;
}
