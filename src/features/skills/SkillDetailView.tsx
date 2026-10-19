import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import { copyToClipboard, Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import PageHeader from "../../layout/PageHeader";
import { displayPath } from "../../pathDisplay";
import { AGENT_LABELS, type Agent, type SkillLocation, type SkillProjectScope, type SkillResourceDetail, type SkillScopeEntry, type SkillSourceKind, type SkillUninstallPreview } from "../../types";
import { locationSupportsAgent } from "./skillPresentation";
import { SKILL_AGENTS } from "./skillAgents";
import SkillConfigDialog from "./SkillConfigDialog";
import SkillRegisterDialog from "./SkillRegisterDialog";
import { skillDetailCache, clearSkillDetailCache } from "./skillDetailCache";
import "./skills.css";

export { skillDetailCache, clearSkillDetailCache };

function AgentIcons({ agents }: { agents: Agent[] }) {
  return <>{agents.map(agent => <span key={agent} role="img" aria-label={AGENT_LABELS[agent]} title={AGENT_LABELS[agent]}><AgentIcon agent={agent} size={16} /></span>)}</>;
}

const SOURCE_KIND_LABELS: Record<SkillSourceKind, string> = {
  library: "NoEnding 源库",
  agent: "Agent 目录",
  external: "外部目录",
};

function scopeAgents(entries: SkillScopeEntry[]) {
  const present = new Set(entries.flatMap(entry => entry.agents));
  return SKILL_AGENTS.filter(agent => present.has(agent));
}

export default function SkillDetailView({ resourceId, initialTitle, navigate }: {
  resourceId: string; initialTitle?: string;
  navigate: (route: Route) => void;
}) {
  const cached = skillDetailCache.get(resourceId);
  const [locations, setLocations] = useState<SkillLocation[]>(() => cached?.locations ?? []);
  const [detail, setDetail] = useState<SkillResourceDetail | null>(() => cached?.detail ?? null);
  const [projectScopes, setProjectScopes] = useState<SkillProjectScope[] | null>(() => cached?.scopes ?? null);
  const [sourceKind, setSourceKind] = useState<SkillSourceKind | null>(() => cached?.source_kind ?? null);
  const [loading, setLoading] = useState(() => !cached);
  const [busyAction, setBusyAction] = useState<"" | "uninstall" | "unregister">("");
  const busy = busyAction !== "";
  const [error, setError] = useState("");
  const [removing, setRemoving] = useState(false);
  const [unregistering, setUnregistering] = useState(false);
  const [removeLinks, setRemoveLinks] = useState(false);
  const [deleteSource, setDeleteSource] = useState(false);
  const [preview, setPreview] = useState<SkillUninstallPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [configuring, setConfiguring] = useState(false);
  const [registering, setRegistering] = useState(false);
  const [contentModalOpen, setContentModalOpen] = useState(false);
  const previewRequest = useRef(0);
  const request = useRef(0);
  const alive = useRef(true);
  const pending = useRef(false);
  const board: Route = { view: "skills" };

  useEffect(() => {
    const fresh = skillDetailCache.get(resourceId);
    if (fresh) {
      setLocations(fresh.locations);
      setDetail(fresh.detail);
      setProjectScopes(fresh.scopes);
      setSourceKind(fresh.source_kind);
      setLoading(false);
    }
  }, [resourceId]);

  const refresh = useCallback(async (silent = false) => {
    const seq = ++request.current;
    if (!silent) setLoading(true);
    try {
      const bundle = await api.getSkillDetail(resourceId);
      if (seq !== request.current || !alive.current) return bundle !== null;
      if (bundle) {
        skillDetailCache.set(resourceId, bundle);
      } else {
        skillDetailCache.delete(resourceId);
      }
      setLocations(bundle?.locations ?? []);
      setDetail(bundle?.detail ?? null);
      setProjectScopes(bundle?.scopes ?? null);
      setSourceKind(bundle?.source_kind ?? null);
      setError(bundle ? "" : "此资源已移除或来源已变化，请返回技能看板。");
      return bundle !== null;
    } catch (e) {
      if (seq === request.current && alive.current) {
        skillDetailCache.delete(resourceId);
        setDetail(null);
        setError(String(e));
      }
      throw e;
    } finally { if (seq === request.current && alive.current && !silent) setLoading(false); }
  }, [resourceId]);
  useEffect(() => {
    alive.current = true;
    void refresh(!!skillDetailCache.get(resourceId)).catch(() => {});
    return () => { alive.current = false; ++request.current; };
  }, [refresh, resourceId]);

  const operation = async (work: () => Promise<{ message: string }>, action: "uninstall" | "unregister" = "uninstall") => {
    if (pending.current) return;
    pending.current = true; setBusyAction(action); setError("");
    try {
      const result = await work();
      if (!alive.current) return;
      showToast(result.message); setRemoving(false); setUnregistering(false);
      skillDetailCache.delete(resourceId);
      if (!await refresh() && alive.current) navigate(board);
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusyAction(""); }
  };
  const row = detail?.resource;
  const deleteRequired = preview?.delete_source_required ?? row?.entry_links.some(link => !link) ?? false;
  const openSkillUninstall = async () => {
    if (!row) return;
    setError(""); setPreview(null); setDeleteSource(row.entry_links.some(link => !link)); setRemoving(true);
    const seq = ++previewRequest.current;
    setPreviewing(true);
    try {
      const value = await api.skillUninstallPreview(row.id);
      if (!alive.current || seq !== previewRequest.current) return;
      setPreview(value); setDeleteSource(value.delete_source_required);
    } catch (e) {
      if (alive.current && seq === previewRequest.current) setError(String(e));
    } finally {
      if (alive.current && seq === previewRequest.current) setPreviewing(false);
    }
  };
  const closeRemove = () => {
    if (busy) return;
    ++previewRequest.current; setPreviewing(false); setRemoving(false); setPreview(null);
  };
  const openSourcePath = () => {
    if (!row?.path) return;
    void api.openPath(row.path).catch(e => setError(String(e)));
  };
  const userEntries = row ? row.entry_paths.map((entry, i) => ({ entry, link: row.entry_links[i] === true, location: locations.find(l => l.id === row.locations[i]) })).filter(entry => entry.location?.scope === "user") : [];
  const userScopeEntries: SkillScopeEntry[] = userEntries.map(entry => ({
    path: entry.entry,
    link: entry.link,
    label: entry.location?.label ?? "",
    agents: entry.location ? SKILL_AGENTS.filter(agent => locationSupportsAgent(entry.location!, agent)) : [],
  }));
  const projects = projectScopes ?? [];
  const enabledProjects = projects.filter(project => project.enabled);
  const canConfigure = !!row && row.registered;

  return <div className="main skill-detail-page">
    <PageHeader title={<><Icon name="skill" /><span>{row?.name ?? initialTitle ?? "技能"}</span></>} actions={<>
      {row && <button className="btn ghost icon-button danger" aria-label="卸载 Skill" title="卸载 Skill" disabled={busy || loading} onClick={() => void openSkillUninstall()}><Icon name="trash" /></button>}
    </>} />
    {error && <div role="alert" className="skill-error">{error}</div>}
    {!row ? null : <div className="detail-layout">
      <div className="detail-main">
        <section className="card detail-section" aria-label="技能详情">
          <div className="row between" style={{ alignItems: "center" }}>
            <h2><Icon name="file" />描述</h2>
            <button
              type="button"
              className="btn small ghost"
              onClick={() => setContentModalOpen(true)}
            >
              查看全文
            </button>
          </div>
          {row.description ? (
            <p className="skill-description">{row.description}</p>
          ) : (
            <p className="small muted">暂无技能描述。</p>
          )}
          {row.error && <div className="skill-error">{row.error}</div>}
        </section>
      </div>
      <aside className="detail-aside">
        <section className="card detail-section" aria-label="源目录">
          <h2><Icon name="folder" />源目录</h2>
          <div className="skill-source-location">
            <span
              role="button"
              tabIndex={0}
              className="mono path-link"
              title={`${displayPath(row.path)} · 点击在文件管理器中打开`}
              onClick={openSourcePath}
              onKeyDown={e => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openSourcePath();
                }
              }}
            >{displayPath(row.path)}</span>
            {sourceKind && <span className="badge">{SOURCE_KIND_LABELS[sourceKind]}</span>}
          </div>
          {sourceKind && sourceKind !== "library" && <div className="skill-migration">
            <div className="row">
              <button className="btn small" disabled={busy || loading} onClick={() => setRegistering(true)}>{row.registered ? "迁移到 NoEnding" : "托管到 NoEnding"}</button>
              {row.registered && <button className="btn small" disabled={busy || loading} onClick={() => { setError(""); setRemoveLinks(false); setUnregistering(true); }}>取消托管</button>}
            </div>
            <span className="small muted">{row.registered ? "已引用原目录，可选择迁入 NoEnding 技能库。" : sourceKind === "agent" ? "源目录位于 Agent 加载位置，托管时可选择是否迁移。" : "源目录位于外部目录，托管时可选择是否迁移。"}</span>
          </div>}
        </section>
        {row && <section className="card detail-section" aria-label="启用范围">
          <div className="row between" style={{ alignItems: "center" }}>
            <h2><Icon name="folder" />启用范围</h2>
            {canConfigure && (
              <button
                type="button"
                className="btn ghost icon-button small"
                title="配置"
                aria-label="配置"
                disabled={loading}
                onClick={() => setConfiguring(true)}
              >
                <Icon name="sliders" />
              </button>
            )}
          </div>
          <p className="small muted">{!row.registered
            ? "尚未托管，托管到 NoEnding 后可在用户级或项目中配置。"
            : "用户级对所有项目生效；项目级按项目添加。"}</p>
          {row.registered && row.entry_links.some(link => !link) && <p className="small muted">物理入口会保持启用，不能直接停用；迁移到 NoEnding 后可通过链接控制。</p>}
          {projectScopes === null ? <p className="small muted" role="status">正在读取启用范围…</p>
            : !userScopeEntries.length && !enabledProjects.length ? <p className="small muted">当前未在任何作用域启用。</p>
            : <dl className="skill-scope-summary">
              {userScopeEntries.length > 0 && <div className="skill-scope-summary-row">
                <dt>用户级{userScopeEntries.some(entry => !entry.link) && <span className="badge" title="该加载入口是真实目录而不是链接；NoEnding 只读，迁移或卸载时才会处理">物理入口</span>}</dt>
                <dd><span className="skill-scope-agents"><AgentIcons agents={scopeAgents(userScopeEntries)} /></span></dd>
              </div>}
              {enabledProjects.map(project => {
                const entries = project.workspaces.flatMap(workspace => workspace.entries);
                return <div className="skill-scope-summary-row" key={project.project_id}>
                  <dt>{project.project_name}{entries.some(entry => !entry.link) && <span className="badge" title="该加载入口是真实目录而不是链接；NoEnding 只读，迁移或卸载时才会处理">物理入口</span>}</dt>
                  <dd><span className="skill-scope-agents"><AgentIcons agents={scopeAgents(entries)} /></span></dd>
                </div>;
              })}
            </dl>}
        </section>}
      </aside>
    </div>}
    {removing && row && <Modal title="卸载 Skill" onClose={closeRemove}>
      <>
        <p>将移除该 Skill 在用户级和所有项目中的全部加载入口。</p>
        <label className="skill-check"><input type="checkbox" checked={deleteSource || deleteRequired} disabled={busyAction === "uninstall" || previewing || deleteRequired} onChange={e => setDeleteSource(e.target.checked)} />同时删除源目录</label>
        {deleteRequired && <p className="small muted">源目录位于 Agent 生效目录，必须删除源文件。</p>}
        <p className="skill-path mono">{displayPath(preview?.source_path ?? row.path)}</p>
      </>
      {error && <div role="alert" className="skill-error">{error}</div>}
      <div className="row skill-dialog-actions"><button className="btn" disabled={busyAction === "uninstall"} onClick={closeRemove}>取消</button><button className="btn danger" disabled={busyAction === "uninstall" || previewing} onClick={() => {
        void operation(() => api.uninstallSkill(row.id, row.revision, deleteSource || deleteRequired));
      }}>{busyAction === "uninstall" ? "处理中…" : previewing ? "读取中…" : deleteSource || deleteRequired ? "删除并卸载" : "仅移除"}</button></div>
    </Modal>}
    {unregistering && row && <Modal title="取消托管" onClose={() => { if (!busy) setUnregistering(false); }}>
      <p>取消托管后，NoEnding 将不再管理该技能，源文件会保留。</p>
      <label className="skill-check"><input type="checkbox" checked={removeLinks} disabled={busy} onChange={e => setRemoveLinks(e.target.checked)} />同时移除所有链接入口</label>
      <p className="small muted">{removeLinks ? "移除用户级及所有已登记项目中的该技能链接入口；物理入口和上级目录链接保持不变，仍可能继续生效。" : "保留所有加载入口，当前已生效的配置继续生效。"}</p>
      {error && <div role="alert" className="skill-error">{error}</div>}
      <div className="row skill-dialog-actions">
        <button className="btn" disabled={busy} onClick={() => setUnregistering(false)}>取消</button>
        <button className="btn danger" disabled={busy} onClick={() => void operation(() => api.unregisterSkill(row.id, removeLinks), "unregister")}>{busyAction === "unregister" ? "处理中…" : "确认取消托管"}</button>
      </div>
    </Modal>}
    {configuring && row && <SkillConfigDialog resource={row} locations={locations}
      onClose={() => setConfiguring(false)} onDone={async () => { try { await refresh(true); } finally { setConfiguring(false); } }} />}
    {registering && row && <SkillRegisterDialog id={row.id} name={row.name} onClose={() => setRegistering(false)}
        onDone={result => {
          setRegistering(false);
          skillDetailCache.delete(resourceId);
          if (result.resource_id === resourceId) void refresh(true).catch(() => {});
          else navigate({ view: "skill", resourceId: result.resource_id, initialTitle: result.name });
        }} />}
    {contentModalOpen && row && detail && (
      <SkillContentModal
        name={row.name}
        content={detail.content}
        onClose={() => setContentModalOpen(false)}
      />
    )}
  </div>;
}

export function parseFrontmatter(raw: string): { key: string; value: string }[] {
  const lines = raw.split(/\r?\n/);
  const pairs: { key: string; value: string }[] = [];
  let currentKey = "";
  let currentValue = "";

  const flush = () => {
    if (currentKey) {
      let val = currentValue.trim();
      if ((val.startsWith('"') && val.endsWith('"')) || (val.startsWith("'") && val.endsWith("'"))) {
        val = val.slice(1, -1);
      }
      pairs.push({ key: currentKey, value: val });
    }
  };

  for (const line of lines) {
    const match = line.match(/^([a-zA-Z0-9_-]+):\s*(.*)$/);
    if (match) {
      flush();
      currentKey = match[1];
      const initial = match[2].trim();
      currentValue = /^[|>][-+]?$/.test(initial) ? "" : initial;
    } else if (currentKey && (line.startsWith("  ") || line.startsWith("\t") || (!line.trim() && currentValue))) {
      currentValue += (currentValue ? "\n" : "") + line.trim();
    }
  }
  flush();
  return pairs;
}

function SkillContentModal({ name, content, onClose }: {
  name: string;
  content: string;
  onClose: () => void;
}) {
  const [preview, setPreview] = useState(true);

  const copyAll = async () => {
    const ok = await copyToClipboard(content);
    showToast(ok ? "已复制" : "复制失败");
  };

  const frontmatterMatch = content.match(/^\uFEFF?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
  const frontmatterRaw = frontmatterMatch ? frontmatterMatch[1] : null;
  const frontmatterPairs = frontmatterRaw ? parseFrontmatter(frontmatterRaw) : [];
  const cleanContent = frontmatterMatch ? content.slice(frontmatterMatch[0].length) : content;

  return (
    <Modal title={`${name} · SKILL.md`} wide onClose={onClose}>
      <div className="settings-seg" role="group" aria-label="显示方式" style={{ marginBottom: 12 }}>
        <button
          type="button"
          className={preview ? "" : "on"}
          aria-pressed={!preview}
          onClick={() => setPreview(false)}
        >
          原文
        </button>
        <button
          type="button"
          className={preview ? "on" : ""}
          aria-pressed={preview}
          onClick={() => setPreview(true)}
        >
          预览
        </button>
      </div>
      {preview ? (
        <div className="skill-markdown skill-modal-content">
          {frontmatterPairs.length > 0 ? (
            <div className="skill-frontmatter-box">
              <dl className="skill-properties">
                {frontmatterPairs.map(({ key, value }) => (
                  <div key={key} style={{ display: "contents" }}>
                    <dt className="mono" style={{ fontWeight: 600 }}>{key}</dt>
                    <dd style={{ whiteSpace: "pre-wrap" }}>{value}</dd>
                  </div>
                ))}
              </dl>
            </div>
          ) : frontmatterRaw ? (
            <pre className="skill-source" style={{ margin: "0 0 16px" }}>{frontmatterRaw}</pre>
          ) : null}
          {cleanContent.trim() ? (
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{ a: ({ children }) => <span>{children}</span>, img: () => null }}
            >
              {cleanContent}
            </ReactMarkdown>
          ) : frontmatterPairs.length === 0 && !frontmatterRaw ? (
            <p className="small muted">暂无内容。</p>
          ) : null}
        </div>
      ) : (
        <pre className="skill-source skill-modal-content" style={{ margin: 0 }}>
          {content}
        </pre>
      )}
      <div className="row" style={{ justifyContent: "flex-end", gap: 8, marginTop: 14 }}>
        <button type="button" className="btn" onClick={copyAll}>复制全文</button>
        <button type="button" className="btn" onClick={onClose}>关闭</button>
      </div>
    </Modal>
  );
}
