import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import CustomSelect from "../../components/CustomSelect";
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
  return (
    <section>
      <div className="settings-section-header">
        <div>
          <h3 style={{ display: "flex", alignItems: "center", gap: 8, margin: 0 }}>
            <Icon name="spark" />
            <span>摘要与偏好提取</span>
          </h3>
          <p className="muted small" style={{ margin: "4px 0 0" }}>
            会话、任务、项目摘要及伴随的偏好提取共用此配置。Agent 默认表示由 CLI 自行决定该参数。
          </p>
        </div>
      </div>
      {!draft ? error
        ? <div role="alert">{error}<button className="btn small" onClick={() => setAttempt(value => value + 1)}>重试</button></div>
        : <p className="muted small" role="status">正在读取提取配置…</p>
        : <>
          <div className="field" style={{ marginBottom: 12 }}>
            <span style={{ fontSize: 13, fontWeight: 500, marginBottom: 4, display: "block" }}>提取 Agent</span>
            <CustomSelect
              ariaLabel="提取 Agent"
              value={draft.agent}
              disabled={busy}
              onChange={(val) => {
                setDraft({ agent: val as ExtractionSettings["agent"], runtime: NATIVE_RUNTIME });
                setError("");
              }}
              options={[
                ...((["codex", "claude_code", "pi"] as const).map((agent) => ({
                  value: agent,
                  label: AGENT_LABELS[agent],
                  icon: "bot",
                }))),
                { value: "none", label: "停用提取" },
              ]}
            />
          </div>
          {draft.agent !== "none" && runtime.options && <>
            <RuntimeFields value={draft.runtime} options={runtime.options} disabled={busy}
              onChange={value => setDraft({ ...draft, runtime: value })} onDiscover={() => void runtime.refresh()} />
            <div className="row" style={{ marginTop: 8 }}>
              <CatalogStatus options={runtime.options} />
              <button className="btn small ghost" disabled={runtime.loading} onClick={() => void runtime.refresh(true)}>刷新模型列表</button>
            </div>
          </>}
          {(error || (draft.agent !== "none" && runtime.error)) && <p role="alert" className="danger">{error || runtime.error}</p>}
          <div className="row" style={{ marginTop: 14 }}>
            <button className="btn small primary" disabled={busy || !changed} onClick={() => void save()}>保存提取设置</button>
            <button className="btn small ghost" disabled={busy || !changed} onClick={() => { setDraft(saved); setError(""); }}>撤销提取设置修改</button>
          </div>
        </>}
    </section>
  );
}
