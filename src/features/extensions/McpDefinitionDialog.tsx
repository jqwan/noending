import { useMemo, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";

const TEMPLATE = '{\n  "command": "",\n  "args": []\n}';

export default function McpDefinitionDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [draft, setDraft] = useState(TEMPLATE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const parsed = useMemo(() => {
    try {
      const value = JSON.parse(draft);
      return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
    } catch { return null; }
  }, [draft]);
  const remote = !!parsed && ("url" in parsed || "serverUrl" in parsed);
  const update = (key: string, value: unknown) => {
    if (parsed) setDraft(JSON.stringify({ ...parsed, [key]: value }, null, 2));
  };
  const switchMode = (mode: "stdio" | "http") => {
    if (!parsed) return;
    const next = { ...parsed };
    for (const key of ["command", "args", "url", "serverUrl", "type"]) delete next[key];
    if (mode === "http") next.url = "https://";
    else { next.command = ""; next.args = []; }
    setDraft(JSON.stringify(next, null, 2));
  };
  const valid = name.trim() !== "" && !!parsed && ("command" in parsed || "url" in parsed || "serverUrl" in parsed);
  const submit = async () => {
    if (!valid || busy || !parsed) return;
    setBusy(true); setError("");
    try {
      const result = await api.saveMcpDefinition(name.trim(), description.trim(), parsed);
      showToast(result.message);
      onDone();
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <Modal title="新增 MCP" onClose={() => { if (!busy) onClose(); }}>
    <div className="extension-form">
      <p className="small muted">定义保存在 NoEnding 中，不写入任何 Agent 配置；在扩展看板或项目详情中启用后再写入对应作用域。</p>
      <label>名称<input type="text" aria-label="MCP 名称" value={name} disabled={busy} onChange={e => setName(e.target.value)} /></label>
      <label>说明<input type="text" aria-label="MCP 说明" value={description} disabled={busy} onChange={e => setDescription(e.target.value)} /></label>
      {parsed && <>
        <label>连接方式<select aria-label="连接方式" value={remote ? "http" : "stdio"} disabled={busy} onChange={e => switchMode(e.target.value as "stdio" | "http")}><option value="stdio">本地进程 · stdio</option><option value="http">远程服务 · HTTP</option></select></label>
        {remote
          ? <label>服务地址<input type="text" aria-label="MCP 服务地址" value={String(parsed.url ?? parsed.serverUrl ?? "")} disabled={busy} onChange={e => update(parsed.serverUrl !== undefined && parsed.url === undefined ? "serverUrl" : "url", e.target.value)} /></label>
          : <><label>启动程序<input type="text" aria-label="MCP 启动程序" value={String(parsed.command ?? "")} disabled={busy} onChange={e => update("command", e.target.value)} /></label><label>启动参数（每行一个）<textarea aria-label="MCP 启动参数" rows={3} value={Array.isArray(parsed.args) ? parsed.args.join("\n") : ""} disabled={busy} onChange={e => update("args", e.target.value ? e.target.value.split("\n") : [])} /></label></>}
      </>}
      <details className="extension-advanced" open={!parsed}><summary>完整配置 · 环境变量、请求头及 JSON</summary><label>服务器配置 · JSON<textarea aria-label="MCP JSON 配置" className="mono extension-json" rows={9} value={draft} disabled={busy} spellCheck={false} onChange={e => setDraft(e.target.value)} /></label></details>
      {error && <div role="alert" className="extension-error">{error}</div>}
      <div className="row extension-dialog-actions"><button className="btn" disabled={busy} onClick={onClose}>取消</button><button className="btn primary" disabled={busy || !valid} onClick={() => void submit()}>{busy ? "处理中…" : "创建定义"}</button></div>
    </div>
  </Modal>;
}
