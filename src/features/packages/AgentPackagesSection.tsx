import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import { AGENT_LABELS, type Agent, type PackageEntry } from "../../types";
import { entryState, packageDetailRoute } from "./packagePresentation";

/** Package list inside one Agent's detail page: the same direct user-level
 *  toggle as the board cards, click-through for everything else. */
export default function AgentPackagesSection({ agent, navigate }: {
  agent: Agent;
  navigate: (route: import("../../app/routes").Route) => void;
}) {
  const [packages, setPackages] = useState<PackageEntry[] | null>(null);
  const [error, setError] = useState("");
  const [togglingId, setTogglingId] = useState<string | null>(null);

  const refresh = useCallback(() => api.listPackages().then(inventory => {
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

  return <section className="card agent-settings-section agent-packages" aria-label="插件包">
    <h2>
      <Icon name="puzzle" />
      <span>插件包</span>
    </h2>
    {error && <div role="alert" className="package-error">{error}</div>}
    {packages === null ? (
      <div className="muted small" style={{ padding: "8px 0" }} role="status">正在读取插件包…</div>
    ) : packages.length === 0 ? (
      <div className="muted small" style={{ padding: "8px 0" }}>
        {AGENT_LABELS[agent]} 未安装任何插件包。安装与导入请前往插件包看板。
      </div>
    ) : (
      <div>
        {packages.map(entry => (
          <div className="agent-package-row" key={entry.id}>
            <button type="button" className="package-open"
              onClick={() => navigate(packageDetailRoute(entry))}
              title={entry.description || entry.name}>
              <Icon name="puzzle" />
              <span className="truncate" style={{ fontWeight: 600 }}>{entry.name}</span>
              {entry.version && <span className="badge">v{entry.version}</span>}
            </button>
            {configurable(entry) && <button
              type="button"
              className={"btn small ghost icon-button" + (entryState(entry) === "enabled" ? " is-on" : " is-off")}
              title={entryState(entry) === "enabled" ? "用户级停用" : "用户级启用"}
              aria-label={entryState(entry) === "enabled" ? "停用" : "启用"}
              disabled={togglingId === entry.id}
              onClick={() => void togglePackage(entry)}
            ><Icon name="power" /></button>}
          </div>
        ))}
      </div>
    )}
  </section>;
}
