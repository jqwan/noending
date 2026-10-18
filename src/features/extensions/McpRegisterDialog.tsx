import { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { ExtensionResource } from "../../types";
import "./extensions.css";

/** Turn a discovered native MCP into a NoEnding definition without touching Agent files. */
export default function McpRegisterDialog({ row, onClose, onDone }: {
  row: ExtensionResource; onClose: () => void; onDone: () => void;
}) {
  const [config, setConfig] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    void api.getExtensionDetail(row.id).then(bundle => {
      if (!live) return;
      if (!bundle?.detail.config) { setError("配置已变化，请刷新后重试。"); return; }
      setConfig(bundle.detail.config);
    }).catch(e => { if (live) setError(String(e)); })
      .finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [row.id]);

  const submit = async () => {
    if (!config || busy) return;
    setBusy(true); setError("");
    try {
      const result = await api.saveMcpDefinition(row.name, row.description, config);
      showToast(result.message);
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return <Modal title="登记到 NoEnding" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted">将这份原生 MCP 配置登记为 NoEnding 定义，之后可在用户级或项目中启停；不会修改 Agent 文件。</p>
    <dl className="extension-properties">
      <dt>名称</dt><dd>{row.name}</dd>
      <dt>配置</dt><dd>{loading ? "读取中…" : <pre className="extension-source">{config ? JSON.stringify(config, null, 2) : "—"}</pre>}</dd>
    </dl>
    {error && <div role="alert" className="extension-error">{error}</div>}
    <div className="row extension-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || loading || !config} onClick={() => void submit()}>{busy ? "登记中…" : "登记"}</button>
    </div>
  </Modal>;
}
