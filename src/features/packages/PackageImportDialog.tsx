import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { AGENT_LABELS, type Agent, type PackageImportPreview } from "../../types";

type Selection = { agent: Agent; plugins: string[] };

export default function PackageImportDialog({ onClose, onDone }: {
  onClose: () => void;
  onDone: (message: string) => void;
}) {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<PackageImportPreview | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [previewing, setPreviewing] = useState(false);
  const [selections, setSelections] = useState<Selection[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const request = useRef(0);

  // Preview whatever is typed once typing settles; errors surface before
  // submit instead of after.
  useEffect(() => {
    const trimmed = path.trim();
    if (!trimmed) {
      ++request.current;
      setPreview(null);
      setPreviewError("");
      setPreviewing(false);
      setSelections([]);
      return;
    }
    setPreviewing(true);
    setPreviewError("");
    const seq = ++request.current;
    const timer = window.setTimeout(() => {
      api.previewImportPackage(trimmed).then(value => {
        if (request.current !== seq) return;
        setPreview(value);
        // Everything is pre-selected: dual-format authors most likely want
        // both installs, and a local marketplace's own list is the natural
        // default batch.
        setSelections(value.options.map(option => ({
          agent: option.agent,
          plugins: option.plugins.map(plugin => plugin.name),
        })));
        setPreviewing(false);
      }).catch(e => {
        if (request.current !== seq) return;
        setPreview(null);
        setSelections([]);
        setPreviewError(String(e));
        setPreviewing(false);
      });
    }, 300);
    return () => window.clearTimeout(timer);
  }, [path]);

  const toggleAgent = (agent: Agent, on: boolean) => setSelections(prev =>
    on ? [...prev, { agent, plugins: pluginsOf(agent, prev) }] : prev.filter(s => s.agent !== agent));
  const togglePlugin = (agent: Agent, plugin: string, on: boolean) => setSelections(prev =>
    prev.map(s => s.agent === agent
      ? { ...s, plugins: on ? [...s.plugins, plugin] : s.plugins.filter(p => p !== plugin) }
      : s));
  const pluginsOf = (agent: Agent, list: Selection[]) => list.find(s => s.agent === agent)?.plugins ?? [];

  const ready = preview !== null && !previewError && !previewing
    && (preview.kind === "marketplace"
      ? selections.some(s => s.plugins.length > 0)
      : selections.length > 0);

  const submit = async () => {
    if (busy || !ready || !preview) return;
    setBusy(true);
    setError("");
    try {
      const result = preview.kind === "marketplace"
        ? await api.installLocalMarketplace(preview.path, selections)
        : await api.installLocalPackage(preview.path, selections.map(s => s.agent));
      onDone(result.message);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const single = preview?.kind === "package" && preview.options.length === 1 ? preview.options[0] : null;

  return <Modal title="导入插件包" onClose={() => { if (!busy) onClose(); }}>
    <p className="package-dialog-hint">
      {preview?.kind === "marketplace"
        ? "该目录是本地插件市场：目录将注册到所选 Agent，需保留在原处；勾选要安装的插件。"
        : "将本地插件包目录安装到所选 Agent。NoEnding 不复制、不修改目录；Pi 引用源目录，Antigravity 复制安装。"}
    </p>
    <label>插件包目录
      <div className="row">
        <input type="text" aria-label="插件包目录" value={path} disabled={busy}
          placeholder="/path/to/plugin-directory"
          onChange={e => setPath(e.target.value)} />
        <button className="btn package-pick-btn" disabled={busy} onClick={async () => {
          const picked = await open({ directory: true, multiple: false, title: "选择插件包目录" });
          if (typeof picked === "string") setPath(picked);
        }}>选择目录</button>
      </div>
    </label>
    {previewError && <div role="alert" className="package-error">{previewError}</div>}
    {preview?.kind === "package" && single && <div className="package-preview" aria-label="导入预览">
      <div className="package-preview-head">
        <Icon name="puzzle" />
        <span className="package-preview-name">{single.name || "未命名插件包"}</span>
        <span className="badge">{single.manifest}</span>
      </div>
      {single.description
        ? <p className="package-preview-desc">{single.description}</p>
        : <p className="small muted">清单未提供描述。</p>}
      <p className="small muted package-preview-agent">将安装到 {AGENT_LABELS[single.agent]}</p>
    </div>}
    {preview?.kind === "package" && !single && <div className="package-preview" aria-label="导入预览">
      <p className="small muted">该目录包含多种清单格式，勾选要安装的：</p>
      {preview.options.map(option => (
        <label key={option.agent} className="package-check">
          <input type="checkbox" checked={selections.some(s => s.agent === option.agent)} disabled={busy}
            onChange={e => toggleAgent(option.agent, e.target.checked)} />
          <span className="package-preview-name">{option.name || option.manifest}</span>
          <span className="badge">{AGENT_LABELS[option.agent]} · {option.manifest}</span>
        </label>
      ))}
    </div>}
    {preview?.kind === "marketplace" && <div className="package-preview" aria-label="导入预览">
      {preview.options.map(option => (
        <div key={option.agent} className="package-market-option">
          <div className="package-preview-head">
            <Icon name="puzzle" />
            <span className="package-preview-name">{option.name || option.manifest}</span>
            <span className="badge">{AGENT_LABELS[option.agent]}</span>
          </div>
          {option.plugins.map(plugin => (
            <label key={plugin.name} className="package-check">
              <input type="checkbox" disabled={busy}
                checked={pluginsOf(option.agent, selections).includes(plugin.name)}
                onChange={e => togglePlugin(option.agent, plugin.name, e.target.checked)} />
              <span className="package-preview-name">{plugin.name}</span>
              {plugin.description && <span className="small muted package-market-plugin-desc">{plugin.description}</span>}
            </label>
          ))}
        </div>
      ))}
    </div>}
    {!preview && !previewError && previewing && <p className="small muted" role="status">正在读取清单…</p>}
    {error && <div role="alert" className="package-error">{error}</div>}
    <div className="row package-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn" disabled={busy || !ready} onClick={() => void submit()}>
        {busy ? "处理中…" : "安装"}
      </button>
    </div>
  </Modal>;
}
