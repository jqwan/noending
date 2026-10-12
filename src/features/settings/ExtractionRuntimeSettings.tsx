import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { AGENT_LABELS, type ExtractionSettings } from "../../types";
import { useAgentRuntime } from "../agents/useAgentRuntime";
import RuntimeFields, { CatalogStatus, NATIVE_RUNTIME } from "../agents/RuntimeFields";

export default function ExtractionRuntimeSettings() {
  const [saved, setSaved] = useState<ExtractionSettings | null>(null);
  const [draft, setDraft] = useState<ExtractionSettings | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const active = useRef(true);
  const runtime = useAgentRuntime(draft?.agent === "none" ? "codex" : (draft?.agent ?? "codex"), Boolean(draft && draft.agent !== "none"));
  useEffect(() => {
    active.current = true;
    setError("");
    let cancelled = false;
    api.getExtractionSettings().then(value => {
      if (!cancelled) { setSaved(value); setDraft(value); }
    }).catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; active.current = false; };
  }, [attempt]);

  const save = async () => {
    if (!draft || busy) return;
    setBusy(true);
    setError("");
    try {
      const value = await api.setExtractionSettings(draft);
      if (active.current) { setSaved(value); setDraft(value); showToast("已保存，下次提取生效"); }
    } catch (e) { if (active.current) setError(String(e)); }
    finally { if (active.current) setBusy(false); }
  };
  const changed = saved && draft && JSON.stringify(saved) !== JSON.stringify(draft);
  return <section>
    <h3>摘要与偏好提取</h3>
    <p className="muted small">会话、任务、项目摘要及伴随的偏好提取共用此配置。Agent 默认表示由 CLI 自行决定该参数。</p>
    {!draft ? error
      ? <div role="alert">{error}<button className="btn small" onClick={() => setAttempt(value => value + 1)}>重试</button></div>
      : <p className="muted small" role="status">正在读取提取配置…</p>
      : <>
        <label className="field"><span>提取 Agent</span><select aria-label="提取 Agent" value={draft.agent} disabled={busy}
          onChange={e => { setDraft({ agent: e.target.value as ExtractionSettings["agent"], runtime: NATIVE_RUNTIME }); setError(""); }}>
          {(["codex", "claude_code", "pi"] as const).map(agent => <option key={agent} value={agent}>{AGENT_LABELS[agent]}</option>)}
          <option value="none">停用提取</option>
        </select></label>
        {draft.agent !== "none" && runtime.options && <>
          <RuntimeFields value={draft.runtime} options={runtime.options} disabled={busy}
            onChange={value => setDraft({ ...draft, runtime: value })} onDiscover={() => void runtime.refresh()} />
          <div className="row">
            <CatalogStatus options={runtime.options} />
            <button className="btn small ghost" disabled={runtime.loading} onClick={() => void runtime.refresh(true)}>刷新模型列表</button>
          </div>
        </>}
        {(error || (draft.agent !== "none" && runtime.error)) && <p role="alert">{error || runtime.error}</p>}
        <div className="row" style={{ marginTop: 12 }}>
          <button className="btn small" disabled={busy || !changed} onClick={() => void save()}>保存提取设置</button>
          <button className="btn small ghost" disabled={busy || !changed} onClick={() => { setDraft(saved); setError(""); }}>撤销提取设置修改</button>
        </div>
      </>}
  </section>;
}
