import { DetailList, DetailListRow, DetailLoading, DetailSection } from "../../components/DetailSection";
import "./packages.css";
import Notice from "../../components/Notice";
import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import { AGENT_LABELS, type Agent, type PackageEntry } from "../../types";
import { entryState, packageDetailRoute } from "./packagePresentation";
import { packageCatalogCache, setPackageCatalogCache } from "./packageDetailCache";

/** Package list inside one Agent's detail page: the same direct user-level
 *  toggle as the board cards, click-through for everything else. */
export default function AgentPackagesSection({ agent, navigate }: {
  agent: Agent;
  navigate: (route: import("../../app/routes").Route) => void;
}) {
  const [packages, setPackages] = useState<PackageEntry[] | null>(() => packageCatalogCache?.packages.filter(entry => entry.agent === agent) ?? null);
  const [error, setError] = useState("");
  const [togglingId, setTogglingId] = useState<string | null>(null);

  const refresh = useCallback(() => api.listPackages().then(inventory => {
    setPackageCatalogCache(inventory);
    setPackages(inventory.packages.filter(entry => entry.agent === agent));
  }).catch(e => setError(String(e))), [agent]);

  useEffect(() => {
    let live = true;
    void refresh().then(() => { if (!live) return; }).catch(() => {});
    return () => { live = false; };
  }, [refresh]);

  const togglePackage = async (entry: PackageEntry) => {
    if (togglingId) return;
    const enable = entryState(entry) !== "enabled";
    setTogglingId(entry.id);
    try {
      const result = enable
        ? await api.enablePackage(entry.id, entry.revision)
        : await api.disablePackage(entry.id, entry.revision);
      showToast(result.message);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setTogglingId(null);
    }
  };

  const configurable = (entry: PackageEntry) =>
    entry.capabilities.toggle && entry.scopes.some(scope => scope.scope === "user");

  return <DetailSection sectionId="packages" label="插件包" className="agent-packages" title={<>
      <Icon name="puzzle" />
      <span>插件包{packages !== null ? `（${packages.length}）` : ""}</span>
    </>} status={<>
      {error && <Notice role="alert" >{error}</Notice>}
    </>}>


    {packages === null ? (
      !error && <DetailLoading>正在读取插件包…</DetailLoading>
    ) : packages.length === 0 ? (
      <div className="muted small" style={{ padding: "8px 0" }}>
        {AGENT_LABELS[agent]} 未安装任何插件包。安装与导入请前往插件包看板。
      </div>
    ) : (
      <DetailList label="插件包列表">
        {packages.map(entry => (
          <DetailListRow key={entry.id} icon={<Icon name="puzzle" />}
            title={entry.name} titleHint={entry.description || entry.name}
            onOpen={() => navigate(packageDetailRoute(entry))}
            meta={entry.version && <span className="badge">v{entry.version}</span>}
            actions={configurable(entry) && <button
              type="button"
              className={"btn small ghost icon-button" + (entryState(entry) === "enabled" ? " is-on" : " is-off")}
              title={entryState(entry) === "enabled" ? "用户级停用" : "用户级启用"}
              aria-label={entryState(entry) === "enabled" ? "停用" : "启用"}
              disabled={togglingId === entry.id}
              onClick={() => void togglePackage(entry)}
            ><Icon name="power" /></button>}
          />
        ))}
      </DetailList>
    )}
  </DetailSection>;
}
