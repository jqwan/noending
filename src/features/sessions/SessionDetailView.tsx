import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { type SessionMessageData } from "./SessionMessage";
import ResumeSessionModal from "./ResumeSessionModal";
import PermanentDeleteModal from "./PermanentDeleteModal";
import {
  agentDisplayLabel,
  bindingRoleLabel,
  formatDateTime,
  primaryFirst,
  projectCellFor,
  sessionDisplayTitle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import { type Project, type SessionDetail, type Workstream } from "../../types";
import type { Route } from "../../app/routes";

/** 后端 get_session_detail 的 events 上限（commands.rs）——到达上限时如实说明。 */
const EVENT_PAGE_LIMIT = 500;

/**
 * Session Detail = 一次具体 Agent 执行的记录（方案 v0.1 §13）。
 * Execution-oriented 而不是 Context-oriented：Header 回答「哪个 Agent、什么时候开始、
 * 最近什么时候动过、在哪个目录、Session ID 是什么」，正文是标准化消息流。
 * 这里不出现同步提取、Context 变更或自动归类。
 *
 * v0.2 增加两行只读事实（方案 §22、§43.3-M29）：Session 的 cwd 解析成的
 * WorkspacePath，以及从那条路径派生出来的 Project。两者都**没有编辑入口**——
 * 路径由 NoEnding 从磁盘观察得到，Project 只有「移动路径」这一条改变方式，
 * 而那属于 Projects 侧，不属于一次已经发生的执行记录。
 */
export default function SessionDetailView({ sessionId, navigate }: {
  sessionId: string;
  navigate: (r: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [bindingOpen, setBindingOpen] = useState(false);
  const [resumeOpen, setResumeOpen] = useState(false);
  const [syncing, setSyncing] = useState(false);
  // 回收站动作（Session Lifecycle & Deletion §36）：确认弹窗、执行中的 busy、
  // 以及从详情页直接发起的永久删除 Modal。
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字——
   * 那是 v0.2 之前手工指派留下的历史标签（§43.4-2）。派生链自带名字，
   * 所以正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => { setDetail(d); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); });
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const needsLegacyName = !detail?.workspace_path && !!detail?.session.project_id;
  useEffect(() => {
    if (!needsLegacyName || projects) return;
    api.listProjects().then(setProjects).catch(console.error);
  }, [needsLegacyName, projects]);
  const projectNameById = useMemo(
    () => new Map((projects ?? []).map((p) => [p.id, p.name])),
    [projects],
  );

  if (failed && !detail) {
    return (
      <div className="main narrow">
        <PageHeader back="Sessions" onBack={() => navigate({ view: "sessions" })} title="读取 Session 失败">
          <p className="muted small">
            这个 Session 可能已经被 Agent 自己清理。本地数据没有被修改，可以重试。
          </p>
          <div className="invite">
            <button className="btn small" onClick={refresh}>重试</button>
          </div>
        </PageHeader>
      </div>
    );
  }
  if (!detail) return <div className="main narrow">加载中…</div>;
  const { session, events, bindings } = detail;

  /**
   * 刷新 = 只摄入。Context 提取只在智能开启时才会发生（方案 v0.1 §11.6），
   * 所以提取结果只跟随后端的 context_processing_enabled 走：智能关闭时它恒为 false，
   * 这一屏永远不会出现「Context 变更」（§13 移除清单）。
   */
  const doSync = async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      const r = await api.syncSession(sessionId);
      const extracted = r.context_processing_enabled && r.applied > 0;
      showToast(
        extracted
          ? `摄入了 ${r.ingested} 条新消息 · 提取了 ${r.applied} 个 Context 变更`
          : r.ingested > 0
            ? `摄入了 ${r.ingested} 条新消息`
            : "没有新消息",
      );
      refresh();
    } catch (e) {
      console.error(e);
      showToast("刷新失败，本地数据没有被修改");
    } finally {
      setSyncing(false);
    }
  };

  const title = sessionDisplayTitle(session.title);
  const untitled = title === UNTITLED_SESSION;
  const cwd = (session.cwd ?? "").trim();
  const ordered = primaryFirst(bindings, ([b]) => b.role);
  /** 单一生命周期权威：null = 正常，时间戳 = 在回收站（§3）。 */
  const trashed = session.trashed_at !== null;

  /**
   * 移入回收站（§36）：全局隐藏，不删任何数据。成功后回到 Sessions 列表——
   * 这个页面展示的执行事实仍然有效，但入口动作（继续 / 刷新）已经不适用。
   */
  const doTrash = async () => {
    if (trashBusy) return;
    setTrashBusy(true);
    try {
      await api.trashSession(sessionId);
      showToast("已移入回收站");
      setConfirmTrash(false);
      navigate({ view: "sessions" });
    } catch (e) {
      console.error(e);
      showToast(`移入回收站失败：${String(e)}`);
      setTrashBusy(false);
    }
  };

  /** 从回收站恢复：trashed_at 清空后本页就地回到正常形态。 */
  const doRestore = async () => {
    if (trashBusy) return;
    setTrashBusy(true);
    try {
      await api.restoreSession(sessionId);
      showToast("已恢复");
      refresh();
    } catch (e) {
      // 后端可能拒绝：例如还有未取消的永久删除任务（须先取消任务）。
      console.error(e);
      showToast(String(e));
    } finally {
      setTrashBusy(false);
    }
  };
  /** 派生链自带的路径与 Project（§43.3-M29：详情只承认这一种真相）。 */
  const workspacePath = detail.workspace_path;
  const legacyProjectCell = projectCellFor(session, projectNameById);
  const derivedProjectName = workspacePath && workspacePath.project_name.trim() !== ""
    ? workspacePath.project_name
    : null;

  const messages: SessionMessageData[] = events.map((e) => ({
    sequence: e.sequence,
    kind: e.kind,
    text: e.text,
    ts: e.ts,
    who: agentDisplayLabel(session.agent),
  }));

  return (
    <div className="main narrow">
      <PageHeader
        back="Sessions"
        onBack={() => navigate({ view: "sessions" })}
        title={title}
        actions={trashed ? (
          // 回收站中的会话：摄入已停止、后端拒绝 Resume——两个入口都如实呈现为不可用。
          <button className="btn primary" disabled
            title="回收站中的会话不能继续；先在上面的横幅里恢复它。">
            继续
          </button>
        ) : (
          <>
            <button className="btn ghost" onClick={doSync} disabled={syncing}>刷新</button>
            <button className="btn primary" onClick={() => setResumeOpen(true)}>继续</button>
          </>
        )}
      >
        <div className="ws-detail-head-meta">
          <span className="row" style={{ gap: 6 }}>
            <AgentIcon agent={session.agent} />
            {agentDisplayLabel(session.agent)}
          </span>
          {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
          {trashed && <span className="badge warn">回收站</span>}
          <span className="dot-sep" />
          <span>{messages.length > 0 ? `${messages.length} 条消息` : "尚无消息"}</span>
        </div>
      </PageHeader>

      {/* 回收站横幅（§36）：替代正常动作区，恢复 / 永久删除都在这里。 */}
      {trashed && (
        <div className="session-trash-banner">
          <div style={{ minWidth: 0 }}>
            <b>该会话在回收站中</b>
            <div className="small muted" style={{ marginTop: 2 }}>
              移入回收站：{formatDateTime(session.trashed_at)}。NoEnding 已停止摄入这个会话；
              Agent 原始会话不会被删除，随时可以恢复。
            </div>
          </div>
          <div className="row" style={{ flex: "none", gap: 8 }}>
            <button className="btn small" disabled={trashBusy} onClick={doRestore}>恢复</button>
            <button className="btn small" disabled={trashBusy} onClick={() => setPurgeOpen(true)}>
              永久删除…
            </button>
          </div>
        </div>
      )}

      {/* 执行事实：一次会话「在什么时候、哪个目录、叫什么 ID」。值可整段选中并复制。 */}
      <div style={{
        display: "grid",
        gridTemplateColumns: "auto minmax(0, 1fr)",
        gap: "7px 16px",
        alignItems: "baseline",
        marginTop: 20,
      }}>
        <Field label="开始时间">
          {session.started_at
            ? <span title={session.started_at}>{formatDateTime(session.started_at)}</span>
            : <span className="muted">未知</span>}
        </Field>
        <Field label="最近活动">
          {session.last_activity_at ? (
            <span title={session.last_activity_at}>
              {timeAgo(session.last_activity_at)}
              <span className="muted small"> · {formatDateTime(session.last_activity_at)}</span>
            </span>
          ) : <span className="muted">未知</span>}
        </Field>
        <Field label="工作目录">
          {cwd
            ? <CopyValue value={cwd} title={`${cwd} · Agent 原始记录里的 cwd，是这条 Session 自己的事实`} />
            : <span className="muted">{NO_CWD}（该 Session 的原始记录里没有目录信息）</span>}
        </Field>
        <Field label="工作路径">
          {workspacePath ? (
            <>
              <CopyValue
                value={workspacePath.canonical_path}
                mono
                title={`${workspacePath.canonical_path} · NoEnding 识别工作位置用的规范化路径`}
              />
              {cwd !== "" && cwd !== workspacePath.canonical_path && (
                <div className="muted small" style={{ marginTop: 4 }}>
                  与上面的「工作目录」写法不同，是因为 NoEnding 按自己的规则把它规范化了。
                </div>
              )}
              {!workspacePath.exists && (
                <div style={{ marginTop: 4 }}>
                  <span
                    className="badge warn"
                    title="最近一次目录检查时在磁盘上找不到这个目录。工作路径的身份由路径本身决定，不靠目录存在与否；目录回来时仍然对上同一条工作路径。"
                  >
                    目录不存在
                  </span>
                </div>
              )}
            </>
          ) : cwd ? (
            <span className="muted small">
              这个目录还没有被登记成工作路径 —— NoEnding 会在下一次目录扫描后自动补上，不需要手工操作。
            </span>
          ) : (
            <span className="muted small">没有工作目录，也就没有工作路径。</span>
          )}
        </Field>
        <Field label="Project">
          {workspacePath ? (
            derivedProjectName ? (
              <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                <button
                  className="link"
                  title={`打开 Project：${derivedProjectName}`}
                  onClick={() => navigate({ view: "project", projectId: workspacePath.project_id })}
                >
                  {derivedProjectName}
                </button>
                <span className="muted small">由上面的工作路径自动派生，只读</span>
              </span>
            ) : (
              <span className="muted small">这条工作路径所属的 Project 记录暂时读不到。</span>
            )
          ) : session.project_id ? (
            <span className="muted small" style={{ wordBreak: "break-word" }}>
              {legacyProjectCell.text}
              {" —— "}
              {legacyProjectCell.hint}
            </span>
          ) : (
            <span className="muted small">
              {cwd === ""
                ? "没有记录过工作目录，所以没有 Project。"
                : "工作路径还没有登记，所以暂时没有 Project。"}
            </span>
          )}
        </Field>
        <Field label="Session ID">
          <CopyValue value={session.id} mono />
        </Field>
        <Field label="Agent 侧 Session ID">
          <CopyValue value={session.agent_session_id} mono
            title="Agent 自己记录里的 Session ID，用于回到原始转录文件" />
        </Field>
      </div>

      <div className="row between" style={{ marginTop: 30, marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>关联 Workstream</div>
        <button className="btn small ghost" onClick={() => setBindingOpen(true)}>编辑</button>
      </div>
      {bindings.length === 0 && (
        <div className="l1-none">
          还没有关联任何 Workstream。点「编辑」可以把它关联到一个或多个 Workstream；
          不关联也可以直接「继续」这个 Session。
        </div>
      )}
      {ordered.map(([b, t]) => (
        <div className="rail-row" key={b.workstream_id}
          onClick={() => navigate({ view: "workstream", workstreamId: b.workstream_id })}
          title={t ?? "这个 Workstream 记录已不存在"}>
          <div className="rail-main">
            <div className="rail-title">{t ?? "Workstream 已不可用"}</div>
            {t === null && <div className="rail-sub mono">{b.workstream_id}</div>}
          </div>
          <span className={`badge ${b.role === "primary" ? "dark" : ""}`}>{bindingRoleLabel(b.role)}</span>
        </div>
      ))}

      <div className="section-label" style={{ marginTop: 34 }}>消息</div>
      <p className="muted small" style={{ margin: "0 0 6px" }}>
        {messages.length >= EVENT_PAGE_LIMIT
          ? `标准化视图，只读。这里显示的是最近读取到的 ${messages.length} 条消息，可能还有更早的消息没进这次读取范围。`
          : "标准化视图，只读。原始数据始终保留在 Agent 自己的目录中。"}
      </p>
      {messages.map((m) => <SessionMessage key={m.sequence} msg={m} />)}
      {messages.length === 0 && (
        <div className="empty">
          还没有摄入消息。
          <div className="small" style={{ marginTop: 4 }}>
            这个 Session 的原始文件还没被读取。点「刷新」立即摄入，或重启应用后自动发现。
          </div>
          <div className="invite">
            <button className="btn small" onClick={doSync} disabled={syncing}>刷新</button>
          </div>
        </div>
      )}

      {/* 危险操作（§36）：只在正常状态下出现；回收站里的动作在顶部横幅。 */}
      {!trashed && (
        <section className="rail-section" style={{ marginTop: 34 }}>
          <div className="section-label">危险操作</div>
          <p className="muted small" style={{ margin: "4px 0 10px", maxWidth: "72ch" }}>
            移入回收站 = 在 NoEnding 中全局隐藏该 Session：不再出现在列表、搜索与继续入口里，
            摄入也会停止。Agent 原始会话不会被删除，随时可以从 Sessions 页的「回收站」恢复。
          </p>
          <button className="btn small" disabled={trashBusy} onClick={() => setConfirmTrash(true)}>
            移入回收站…
          </button>
        </section>
      )}

      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => { if (!trashBusy) setConfirmTrash(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会从 Sessions 列表、搜索与继续入口中消失，出现在 Sessions 页的「回收站」里。
          </p>
          {/* §36 要求把两个概念摆在同一处明确区分：「从 Workstream 移除」只改
              Workstream 成员关系（在「编辑关联」弹窗里），这里是全局回收站。 */}
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从 Workstream 移除</b> = 只修改这个 Workstream 的成员关系。
            </p>
            <p style={{ margin: 0 }}>
              <b>移入回收站</b> = 在 NoEnding 中全局隐藏该 Session。Agent 原始会话不会被删除。
            </p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmTrash(false)} disabled={trashBusy}>取消</button>
            <button className="btn primary" onClick={doTrash} disabled={trashBusy}>
              {trashBusy ? "处理中…" : "移入回收站"}
            </button>
          </div>
        </Modal>
      )}

      {purgeOpen && (
        <PermanentDeleteModal
          sessionId={sessionId}
          onClose={() => { setPurgeOpen(false); refresh(); }}
          onDeleted={() => { setPurgeOpen(false); navigate({ view: "sessions" }); }}
        />
      )}

      {bindingOpen && (
        <BindingModal
          sessionId={sessionId}
          bindings={bindings.map(([b]) => b)}
          onClose={() => setBindingOpen(false)}
          onChanged={refresh}
        />
      )}
      {resumeOpen && (
        <ResumeSessionModal
          sessionId={sessionId}
          onClose={() => setResumeOpen(false)}
        />
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <div className="muted small" style={{ whiteSpace: "nowrap" }}>{label}</div>
      <div style={{ minWidth: 0 }}>{children}</div>
    </>
  );
}

/** 值 + 复制：报障时用户要能原样给出 Session ID 与工作目录。 */
function CopyValue({ value, mono, title }: { value: string; mono?: boolean; title?: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    const ok = await copyToClipboard(value);
    setCopied(ok);
    showToast(ok ? "已复制到剪贴板" : "复制失败，请手动选中文字复制");
    if (ok) setTimeout(() => setCopied(false), 2500);
  };

  return (
    <span className="row" style={{ gap: 8 }}>
      <span
        className={mono ? "mono" : undefined}
        title={title ?? value}
        style={{ minWidth: 0, wordBreak: "break-all", userSelect: "all" }}
      >
        {value}
      </span>
      <button className="link" style={{ flex: "none" }} onClick={copy}>
        {copied ? "已复制" : "复制"}
      </button>
    </span>
  );
}

/**
 * 剪贴板：Tauri webview 里 navigator.clipboard 通常可用（localhost / tauri:// 都是
 * secure context），但没有授权时会抛；退到隐藏 textarea + execCommand，最后返回
 * false 让调用方给出「手动选中」的提示，绝不静默失败。
 */
async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch (e) {
    console.error(e);
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch (e) {
    console.error(e);
    return false;
  }
}

/** Binding 编辑（实施方案 §36）：主关联 / 相关关联 / 移除 / 添加，Modal 即可。 */
function BindingModal({ sessionId, bindings, onClose, onChanged }: {
  sessionId: string;
  bindings: { workstream_id: string; role: string }[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [rows, setRows] = useState(bindings.map((b) => ({ ...b })));
  const [addId, setAddId] = useState("none");
  const [addRole, setAddRole] = useState("related");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api.listWorkstreams().then(setWorkstreams).catch(console.error);
  }, []);

  const apply = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      // 后端在单个事务里做 diff：未改动的 Binding 原 row 保留
      // （provenance / created_at / cursor 不动），只有新增的才是 user_assigned。
      await api.replaceSessionBindings(
        sessionId,
        rows.map((r) => ({ workstream_id: r.workstream_id, role: r.role })),
      );
      onChanged();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const bound = new Set(rows.map((r) => r.workstream_id));
  const titleOf = (id: string) => workstreams.find((w) => w.id === id)?.title ?? null;

  return (
    <Modal title="编辑关联 Workstream" onClose={onClose}>
      {/* v0.2：Session 归类只作用在 Workstream 上（方案 §22「不让用户操作 Project」）。
          Project 是工作目录的派生结果，在这里出现只会让人以为它可以被指派。 */}
      <div className="muted small" style={{ marginBottom: 10 }}>
        这里只操作 Workstream。Project 由这条 Session 自己的工作目录派生，不需要、也不能在这里选。
      </div>
      {rows.length === 0 && (
        <div className="muted small" style={{ marginBottom: 10 }}>
          还没有关联任何 Workstream。用下面的「添加 Workstream」选择。
        </div>
      )}
      {rows.map((r) => {
        const name = titleOf(r.workstream_id);
        return (
          <div className="row between" key={r.workstream_id}
            style={{ borderTop: "1px solid var(--bg-panel)", padding: "9px 0", gap: 12 }}>
            <div style={{ fontSize: 13.5, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {name ?? (
                <span title="这个 Workstream 记录已不存在，仍保留其 ID 以便追溯">
                  <span className="muted">Workstream 已不可用</span>
                  <span className="mono" style={{ marginLeft: 6 }}>{r.workstream_id}</span>
                </span>
              )}
            </div>
            <div className="row" style={{ flex: "none" }}>
              <select style={{ width: 110 }} value={r.role}
                onChange={(e) => setRows((rs) => rs.map((x) => x.workstream_id === r.workstream_id ? { ...x, role: e.target.value } : x))}>
                <option value="primary">主关联</option>
                <option value="related">相关关联</option>
              </select>
              <button className="btn small ghost"
                onClick={() => setRows((rs) => rs.filter((x) => x.workstream_id !== r.workstream_id))}>
                移除
              </button>
            </div>
          </div>
        );
      })}

      <div className="row between"
        style={{ borderTop: "1px solid var(--bg-panel)", padding: "9px 0", gap: 12 }}>
        <div style={{ fontSize: 13.5 }}>添加 Workstream</div>
        <div className="row" style={{ flex: "none" }}>
          <select style={{ width: 200 }} value={addId} onChange={(e) => setAddId(e.target.value)}>
            <option value="none">选择…</option>
            {workstreams.filter((w) => !bound.has(w.id) && w.visibility === "normal").map((w) => (
              <option key={w.id} value={w.id}>{w.title}</option>
            ))}
          </select>
          <select style={{ width: 110 }} value={addRole} onChange={(e) => setAddRole(e.target.value)}>
            <option value="related">相关关联</option>
            <option value="primary">主关联</option>
          </select>
          <button className="btn small" disabled={addId === "none"}
            onClick={() => { setRows((rs) => [...rs, { workstream_id: addId, role: addRole }]); setAddId("none"); }}>
            添加
          </button>
        </div>
      </div>

      {/* 这一侧的效果要说清楚（方案 §1.8 / §1.9）：关联一次会顺带决定
          Workstream 的工作路径列表，而移除留下的否定决定是持久的。 */}
      <div className="muted small" style={{ marginTop: 12, wordBreak: "break-word" }}>
        保存时会发生什么：新加的关联，如果那个 Workstream 的工作路径列表里还没有这条 Session 的工作路径，
        那条路径会被追加进去（该 Workstream 还没有任何路径时成为主路径）；
        这条 Session 没有可解析的工作目录时只建立关联，不会编造路径。
        移除只解除关联，路径会留在列表里；同时这是一次明确的否定决定 ——
        NoEnding 之后不会再把这条 Session 自动归类回这个 Workstream，除非你在这里重新关联它。
      </div>

      {error && <div className="badge warn" style={{ marginTop: 8 }}>{error}</div>}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy} onClick={apply}>{busy ? "保存中…" : "保存"}</button>
      </div>
    </Modal>
  );
}
