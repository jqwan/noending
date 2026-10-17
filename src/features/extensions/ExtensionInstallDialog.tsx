import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { AGENT_LABELS, type Agent, type ExtensionKind, type ExtensionLocation, type ExtensionResourceDetail, type ExtensionScope, type WorkspacePath } from "../../types";
import { EXTENSION_AGENTS as AGENTS } from "./extensionAgents";

export default function ExtensionInstallDialog({ kind, scope, scopeName, paths, locations, agent: initialAgent, detail, onClose, onDone }: {
  kind: ExtensionKind; scope: ExtensionScope; scopeName: string; locations: ExtensionLocation[]; agent: Agent;
  paths: WorkspacePath[]; detail: ExtensionResourceDetail | null; onClose: () => void; onDone: () => void;
}) {
  const [locationId, setLocationId] = useState(locations[0]?.id ?? "");
  const [agent, setAgent] = useState<Agent>(initialAgent);
  const [source, setSource] = useState("");
  const [installMode, setInstallMode] = useState<"copy" | "link">("copy");
  const [name, setName] = useState(detail?.resource.name ?? "");
  const [draft, setDraft] = useState(detail?.content ?? '{\n  "command": "",\n  "args": []\n}');
  const [revision, setRevision] = useState(detail?.resource.revision ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [extraPaths, setExtraPaths] = useState<string[]>([]);
  const [outcomes, setOutcomes] = useState<Record<string, { ok: boolean; text: string }>>({});
  const editingLocked = busy || Object.keys(outcomes).length > 0;
  const location = locations.find(l => l.id === locationId);
  const effectiveAgent = location?.agent ?? agent;
  useEffect(() => {
    if (kind !== "mcp" || detail || !locationId) return;
    let live = true; setRevision("");
    void api.extensionLocationRevision(scope, locationId).then(r => { if (live) setRevision(r); }).catch(e => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [kind, detail, locationId, scope]);
  const parsed = useMemo(() => { try { const value = JSON.parse(draft); return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null; } catch { return null; } }, [draft]);
  const update = (key: string, value: unknown) => { if (parsed) setDraft(JSON.stringify({ ...parsed, [key]: value }, null, 2)); };
  const remoteKey = effectiveAgent === "antigravity" ? "serverUrl" : "url";
  const remote = !!parsed && ("url" in parsed || "serverUrl" in parsed);
  const supported = kind !== "package" || scopeName === "user" || agent === "claude_code" || agent === "pi";
  const submit = async () => {
    setBusy(true); setError("");
    try {
      if (kind === "package") {
        const result = await api.managePluginPackage(scope, agent, "install", source);
        showToast(result.message); onDone(); return;
      }
      const targets = [scope.workspace_path_id ?? "user", ...extraPaths];
      let failed = false;
      for (const [index, target] of targets.entries()) {
        if (outcomes[target]?.ok) continue;
        try {
          const targetScope = index === 0 ? scope : { project_id: scope.project_id, workspace_path_id: target };
          let destinationId = locationId;
          let expected = revision;
          if (index > 0) {
            const list = await api.listExtensionResources(targetScope);
            const dest = list.locations.find(l => l.scope === "project" && l.writable && !l.package && l.kind === kind && l.agent === location?.agent && l.label === location?.label);
            if (!dest) throw new Error("目录中没有对应的配置位置");
            destinationId = dest.id;
            if (kind === "mcp") expected = await api.extensionLocationRevision(targetScope, destinationId);
          }
          const result = kind === "skill" ? await api.installExtensionSkill(targetScope, source, destinationId, installMode)
            : await api.saveExtensionMcp(targetScope, destinationId, expected, name, parsed!, index > 0 || !detail);
          setOutcomes(current => ({ ...current, [target]: { ok: true, text: result.message } }));
        } catch (e) { failed = true; setOutcomes(current => ({ ...current, [target]: { ok: false, text: String(e) } })); }
      }
      if (!failed) { showToast("配置操作已完成；已有终端请重新加载或重新打开"); onDone(); }
      else setError("部分目录操作失败，成功项已保留。重试只处理失败项，或关闭后刷新列表。");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <Modal title={kind === "mcp" ? detail ? "编辑 MCP 配置" : "新增 MCP" : kind === "skill" ? "安装 Skill" : "安装插件包"} wide onClose={() => { if (!busy) onClose(); }}>
    <div className="extension-form">
      {kind !== "package" ? <label>配置位置<select aria-label="配置位置" value={locationId} disabled={busy || !!detail || Object.keys(outcomes).length > 0} onChange={e => setLocationId(e.target.value)}>{locations.map(l => <option key={l.id} value={l.id}>{l.label} · {l.path}</option>)}</select></label> : <label>Agent<select aria-label="安装到 Agent" value={agent} disabled={editingLocked} onChange={e => setAgent(e.target.value as Agent)}>{AGENTS.map(a => <option key={a} value={a}>{AGENT_LABELS[a]}</option>)}</select></label>}
      {!detail && kind !== "package" && scope.project_id && paths.some(p => p.exists && p.id !== scope.workspace_path_id) && <fieldset className="extension-targets"><legend>同时新增到其他目录（可选，不覆盖同名项）</legend>{paths.filter(p => p.exists && p.id !== scope.workspace_path_id).map(p => <label className="extension-check" key={p.id}><input type="checkbox" checked={extraPaths.includes(p.id)} disabled={busy || Object.keys(outcomes).length > 0} onChange={e => setExtraPaths(current => e.target.checked ? [...current, p.id] : current.filter(id => id !== p.id))} />{p.canonical_path}</label>)}</fieldset>}
      {kind === "skill" && <><label>安装方式<select aria-label="Skill 安装方式" value={installMode} disabled={editingLocked} onChange={e => setInstallMode(e.target.value as "copy" | "link")}><option value="copy">复制目录 · 独立安装</option><option value="link">链接目录 · 共用源文件</option></select></label><label>源 Skill 目录<div className="row"><input type="text" aria-label="源 Skill 目录" value={source} onChange={e => setSource(e.target.value)} disabled={busy} /><button className="btn" disabled={busy} onClick={async () => { const picked = await open({ directory: true, multiple: false, title: "选择包含 SKILL.md 的目录" }); if (typeof picked === "string") setSource(picked); }}>选择目录</button></div></label><p className="small muted">{installMode === "copy" ? "复制整个 Skill 目录，保留原文件。" : "建立目录链接，源文件修改会影响所有使用它的入口；请保留源目录。"}共享安装会被支持该路径的 Agent 发现，不会覆盖已有目录。</p></>}
      {kind === "package" && <><label>插件包目标<input type="text" aria-label="插件包目标" placeholder={agent === "pi" ? "npm:包名、git:地址或本地目录" : agent === "antigravity" ? "本地插件包目录或原生 CLI 支持的目标" : "插件包名@已配置的市场"} value={source} disabled={editingLocked} onChange={e => setSource(e.target.value)} /></label><p className="small muted">调用 Agent 原生安装命令。插件市场、账号认证及安装依赖由 Agent 管理。</p>{!supported && <p role="alert" className="extension-error">此 Agent 的安装入口只支持用户级，请切换到用户级安装。</p>}</>}
      {kind === "mcp" && <>
        <label>名称<input type="text" aria-label="MCP 名称" value={name} disabled={editingLocked || !!detail} onChange={e => setName(e.target.value)} /></label>
        {parsed && <><label>连接方式<select aria-label="连接方式" value={remote ? "http" : "stdio"} disabled={editingLocked} onChange={e => {
          const next = { ...parsed }; delete next.command; delete next.args; delete next.url; delete next.serverUrl; delete next.type;
          if (e.target.value === "http") { next[remoteKey] = "https://"; if (effectiveAgent === "claude_code") next.type = "http"; }
          else { next.command = ""; next.args = []; }
          setDraft(JSON.stringify(next, null, 2));
        }}><option value="stdio">本地进程 · stdio</option><option value="http">远程服务 · HTTP</option></select></label>
          {remote ? <label>服务地址<input type="text" aria-label="MCP 服务地址" value={String(parsed[remoteKey] ?? "")} disabled={editingLocked} onChange={e => update(remoteKey, e.target.value)} /></label> : <><label>启动程序<input type="text" aria-label="MCP 启动程序" value={String(parsed.command ?? "")} disabled={editingLocked} onChange={e => update("command", e.target.value)} /></label><label>启动参数（每行一个）<textarea aria-label="MCP 启动参数" rows={3} value={Array.isArray(parsed.args) ? parsed.args.join("\n") : ""} disabled={editingLocked} onChange={e => update("args", e.target.value ? e.target.value.split("\n") : [])} /></label></>}
        </>}
        <details className="extension-advanced"><summary>高级配置 · 环境变量、请求头及 JSON</summary><label>完整服务器配置 · JSON<textarea aria-label="MCP JSON 配置" className="mono extension-json" rows={10} value={draft} disabled={editingLocked} spellCheck={false} onChange={e => setDraft(e.target.value)} /></label></details>
        <p className="small muted">环境变量、请求头及 Agent 专用选项可在 JSON 中配置。只修改当前服务器，保留其他配置；上一个文件版本保存为同目录备份。保存不启动或连接服务器。</p>
      </>}
      {error && <div role="alert" className="extension-error">{error}</div>}
      {Object.entries(outcomes).map(([id, result]) => <p className="small extension-path" key={id}>{paths.find(p => p.id === id)?.canonical_path ?? "用户级"} · {result.ok ? "成功" : "失败"}：{result.text}</p>)}
      <div className="row" style={{ justifyContent: "flex-end" }}><button className="btn" disabled={busy} onClick={onClose}>取消</button><button className="btn primary" disabled={busy || !supported || (kind === "mcp" ? !locationId || !revision || !name.trim() || !parsed : !source.trim() || kind === "skill" && !locationId)} onClick={() => void submit()}>{busy ? "处理中…" : kind === "mcp" ? "保存" : "安装"}</button></div>
    </div>
  </Modal>;
}
