import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { type SessionMessageData } from "./SessionMessage";
import ResumeSessionModal from "./ResumeSessionModal";
import {
  agentDisplayLabel,
  bindingRoleLabel,
  formatDateTime,
  primaryFirst,
  sessionDisplayTitle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import { type SessionDetail, type Workstream } from "../../types";
import type { Route } from "../../app/routes";

/** 后端 get_session_detail 的 events 上限（commands.rs）——到达上限时如实说明。 */
const EVENT_PAGE_LIMIT = 500;

/**
 * Session Detail = 一次具体 Agent 执行的记录（方案 v0.1 §13）。
 * Execution-oriented 而不是 Context-oriented：Header 回答「哪个 Agent、什么时候开始、
 * 最近什么时候动过、在哪个目录、Session ID 是什么」，正文是标准化消息流。
 * 这里不出现同步提取、Context 变更或自动归类。
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

  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => { setDetail(d); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); });
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

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
   * 刷新 = 只摄入。Context 提取只在智能开启时才会发生（方案 v0.1 §11.6 + Commit 0 L4），
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
        actions={
          <>
            <button className="btn ghost" onClick={doSync} disabled={syncing}>刷新</button>
            <button className="btn primary" onClick={() => setResumeOpen(true)}>继续</button>
          </>
        }
      >
        <div className="ws-detail-head-meta">
          <span className="row" style={{ gap: 6 }}>
            <AgentIcon agent={session.agent} />
            {agentDisplayLabel(session.agent)}
          </span>
          {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
          <span className="dot-sep" />
          <span>{messages.length > 0 ? `${messages.length} 条消息` : "尚无消息"}</span>
        </div>
      </PageHeader>

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
            ? <CopyValue value={cwd} title={cwd} />
            : <span className="muted">{NO_CWD}（该 Session 的原始记录里没有目录信息）</span>}
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

      {error && <div className="badge warn" style={{ marginTop: 8 }}>{error}</div>}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy} onClick={apply}>{busy ? "保存中…" : "保存"}</button>
      </div>
    </Modal>
  );
}
