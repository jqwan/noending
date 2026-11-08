import { DetailLayout, DetailPanelProvider } from "../../components/DetailLayout";
import { DetailContent, DetailFields, DetailSection } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import Notice from "../../components/Notice";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import { Field, Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import PageHeader from "../../layout/PageHeader";
import { AGENT_LABELS, type Agent, type PackageDetail } from "../../types";
import { originLabel, shortVersion } from "./packagePresentation";
import { packageDetailCache } from "./packageDetailCache";
import "./packages.css";

type BusyAction = "" | "uninstall" | "toggle";

export default function PackageDetailView({ packageId, initialTitle, navigate }: {
  packageId: string; initialTitle?: string; agent?: Agent;
  navigate: (route: Route) => void;
}) {
  const cached = packageDetailCache.get(packageId);
  const [detail, setDetail] = useState<PackageDetail | null>(() => cached ?? null);
  const [loading, setLoading] = useState(() => !cached);
  const [error, setError] = useState("");
  const [uninstalling, setUninstalling] = useState(false);
  const [busyAction, setBusyAction] = useState<BusyAction>("");
  const request = useRef(0);
  const alive = useRef(true);
  const board: Route = { view: "packages" };
  const entry = detail?.entry ?? null;
  const busy = busyAction !== "";

  const refresh = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent) setLoading(true);
    try {
      const value = await api.getPackageDetail(packageId);
      if (seq !== request.current || !alive.current) return value !== null;
      if (value) packageDetailCache.set(packageId, value);
      else packageDetailCache.delete(packageId);
      setDetail(value);
      setError(value ? "" : "此插件包已移除，请返回插件包看板。");
      return value !== null;
    } catch (e) {
      if (seq === request.current && alive.current) {
        setError(String(e));
      }
      return false;
    } finally {
      if (seq === request.current && alive.current && !silent) setLoading(false);
    }
  }, [packageId]);
  useEffect(() => {
    alive.current = true;
    void refresh(!!packageDetailCache.get(packageId));
    return () => { alive.current = false; ++request.current; };
  }, [refresh, packageId]);

  const operate = async (action: BusyAction, work: () => Promise<{ message: string }>, goneAfter = false) => {
    if (busy) return;
    setBusyAction(action);
    setError("");
    try {
      const result = await work();
      if (!alive.current) return;
      showToast(result.message);
      setUninstalling(false);
      if (goneAfter) {
        packageDetailCache.delete(packageId);
        navigate(board);
      } else {
        await refresh(true);
      }
    } catch (e) {
      if (alive.current) setError(String(e));
    } finally {
      if (alive.current) setBusyAction("");
    }
  };

  const uninstall = () => entry && operate("uninstall", () => api.uninstallPackage(entry.id), true);

  const openPath = (path: string) => {
    void api.openPath(path).catch(e => setError(String(e)));
  };

  const userScope = entry?.scopes.find(scope => scope.scope === "user") ?? null;
  const configurable = !!userScope && !!entry?.capabilities.toggle;

  const toggle = (enable: boolean) => entry && operate("toggle", () =>
    enable ? api.enablePackage(entry.id, entry.revision) : api.disablePackage(entry.id, entry.revision));
  return <DetailPanelProvider kind="package"><div className="main package-detail-page detail-page">
    <PageHeader title={entry?.name ?? initialTitle ?? "插件包"} icon={<Icon name="puzzle" />} actions={<>
      {configurable && userScope && <button className={"btn ghost icon-button" + (userScope.state === "enabled" ? " is-on" : " is-off")}
        title={userScope.state === "enabled" ? "用户级停用" : "用户级启用"}
        aria-label={userScope.state === "enabled" ? "停用" : "启用"}
        disabled={busy || loading}
        onClick={() => toggle(userScope.state !== "enabled")}
      ><Icon name="power" /></button>}
      {entry && entry.capabilities.uninstall && <button className="btn ghost icon-button danger" aria-label="卸载插件包" title="卸载插件包" disabled={busy || loading} onClick={() => setUninstalling(true)}><Icon name="trash" /></button>}
    </>} />
    {error && <Notice role="alert" >{error}</Notice>}
    {!entry ? (loading ? <p className="small muted" role="status">正在读取插件包…</p> : null) : <DetailLayout panel={<><DetailSection sectionId="properties" label="属性" title={<><Icon name="file" />属性</>}>

          <DetailFields>
            <Field label="Agent">
              {entry.agent
                ? <button type="button" className="link" title={`打开 Agent：${AGENT_LABELS[entry.agent]}`}
                    onClick={() => navigate({ view: "agents", agent: entry.agent! })}>
                    <span>{AGENT_LABELS[entry.agent]}</span>
                  </button>
                : <span className="muted small">未安装到任何 Agent</span>}
            </Field>
            <Field label="来源">
              <span>{originLabel(entry)}</span>
            </Field>
            {entry.version && <Field label="版本">
              <span className="mono" title={shortVersion(entry.version) !== entry.version ? `完整版本号：${entry.version}` : undefined}>
                {shortVersion(entry.version)}
              </span>
            </Field>}
            {detail?.root && <Field label="安装位置" copyValue={detail.root} copyTitle="复制安装位置">
              <button type="button" className="link mono"
                title={`${detail.root} · 点击在文件管理器中打开`}
                onClick={() => openPath(detail.root!)}>
                <span>{detail.root}</span>
              </button>
            </Field>}
            {entry.scopes.map((scope, index) => <Field label="配置文件" key={index}
              copyValue={scope.config_exists ? scope.config_path : null} copyTitle="复制配置文件路径">
              {scope.config_exists ? <button type="button" className="link mono"
                title={`${scope.config_path} · 启停状态来源，点击在文件管理器中打开`}
                onClick={() => openPath(scope.config_path)}>
                <span>{scope.config_path}</span>
              </button> : <span className="muted small" title="此作用域从未显式设置，配置文件尚未创建；首次启停时会自动创建">
                <span>{scope.config_path}（尚未创建）</span>
              </span>}
            </Field>)}
          </DetailFields>
        </DetailSection></>}><DetailSection sectionId="description" label="插件包详情" title={<><Icon name="info" />描述</>}>

          {entry.description
            ? <DetailContent label="插件包描述内容"><p className="detail-description">{entry.description}</p></DetailContent>
            : <p className="small muted">暂无描述。</p>}
          {entry.error && <Notice>{entry.error}</Notice>}
        </DetailSection>
<DetailSection sectionId="contents" label="包内内容" title={<><Icon name="folder" />包内内容</>}>

          {detail && detail.contents.skills.length === 0 && detail.contents.mcp_files.length === 0
            ? <p className="small muted">未识别到包内 Skill 或 MCP 配置。</p>
            : <>
              {detail && detail.contents.skills.length > 0 && <div className="package-contents">
                {detail.contents.skills.map(skill => <span key={skill} className="badge" title="包内 Skill（由所属 Agent 加载，NoEnding 只读展示）">Skill · {skill}</span>)}
              </div>}
              {detail && detail.contents.mcp_files.length > 0 && <div className="package-contents">
                {detail.contents.mcp_files.map(file => <span key={file} className="badge" title="包内 MCP 配置（由所属 Agent 加载，NoEnding 只读展示）">MCP · {file}</span>)}
              </div>}
              <p className="small muted">包内资源由提供方 Agent 加载，NoEnding 只读展示。</p>
            </>}
        </DetailSection></DetailLayout>}
    {uninstalling && entry && <Modal title="卸载插件包" onClose={() => { if (!busy) setUninstalling(false); }}>
      <p>将通过 {entry.agent ? AGENT_LABELS[entry.agent] : "该 Agent"} 的原生命令卸载该插件包。</p>
      <p className="small muted">注册表中的安装条目随卸载一起消失，作用域内的启停配置不再生效；已有终端需要重新加载。</p>
      {error && <Notice role="alert" >{error}</Notice>}
      <DialogActions>
        <button className="btn" disabled={busy} onClick={() => setUninstalling(false)}>取消</button>
        <button className="btn danger" disabled={busy} onClick={() => void uninstall()}>{busyAction === "uninstall" ? "处理中…" : "卸载"}</button>
      </DialogActions>
    </Modal>}
  </div></DetailPanelProvider>;
}
