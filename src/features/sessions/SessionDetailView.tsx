import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, copyToClipboard, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionMessage, { type SessionMessageData } from "./SessionMessage";
import ResumeSessionModal from "./ResumeSessionModal";
import PermanentDeleteModal from "./PermanentDeleteModal";
import {
  agentDisplayLabel,
  formatDateTime,
  projectCellFor,
  sessionDisplayTitle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import { type Project, type Session, type SessionDetail, type Workstream } from "../../types";
import type { Route } from "../../app/routes";

/** 后端 get_session_detail 的 events 上限（commands.rs）——到达上限时如实说明。 */
const EVENT_PAGE_LIMIT = 500;

/**
 * Session Detail = 一次具体 Agent 执行的记录（方案 v0.1 §13）。
 * Execution-oriented 而不是 Context-oriented：Header 回答「哪个 Agent、什么时候开始、
 * 最近什么时候动过、在哪个目录、Session ID 是什么」，正文是标准化消息流。
 * 这里不出现同步提取、Context 变更或自动归类。
 *
 * v0.2 增加只读事实（方案 §22、§43.3-M29）：Session 的 cwd 解析成的
 * WorkspacePath，以及从那条路径派生出来的 Project。两者都**没有编辑入口**——
 * 路径由 NoEnding 从磁盘观察得到，Project 只有「移动路径」这一条改变方式，
 * 而那属于 Projects 侧，不属于一次已经发生的执行记录。
 * 界面上两者合并为一行「工作目录」：主值是规范化路径，原始 cwd 只在写法
 * 不同时以小字副行出现——两套拼写只在真的不同时才值得同时可见。
 */
export default function SessionDetailView({ sessionId, navigate, goBack }: {
  sessionId: string;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [ownerOpen, setOwnerOpen] = useState(false);
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

  /**
   * 没拿到数据时也要把 PageHeader 画出来（照 ProjectDetail 的做法）：标题行是吸在
   * 应用标题栏那条 band 上的，加载态若不渲染它，整条标题栏会先消失再补回来——那一下
   * 比"内容区里一行加载中"显眼得多。所以这里只换标题文案，不换页面骨架。
   */
  if (!detail) {
    return (
      <div className="main narrow">
        <PageHeader title={failed ? "读取会话失败" : "加载中…"}>
          {failed && (
            <>
              <p className="muted small">
                这个 Session 可能已经被 Agent 自己清理。本地数据没有被修改，可以重试。
              </p>
              <div className="invite">
                <button className="btn small" onClick={refresh}>重试</button>
              </div>
            </>
          )}
        </PageHeader>
      </div>
    );
  }
  const { session, events, owner_workstream } = detail;

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
  /** 单一生命周期权威：null = 正常，时间戳 = 在回收站（§3）。 */
  const trashed = session.trashed_at !== null;

  /**
   * 移入回收站（§36）：全局隐藏，不删任何数据。成功后返回上一个界面——
   * 这个页面展示的执行事实仍然有效，但入口动作（继续 / 刷新）已经不适用。
   */
  const doTrash = async () => {
    if (trashBusy) return;
    setTrashBusy(true);
    try {
      await api.trashSession(sessionId);
      showToast("已移入回收站");
      setConfirmTrash(false);
      goBack({ view: "sessions" });
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
  /** 这条会话「属于」哪个项目：派生链优先，退回 v0.2 之前手工指派留下的 project_id。 */
  const sessionProjectId = workspacePath?.project_id ?? session.project_id ?? null;

  /**
   * 设置所属任务（方案 §29）：一次提交一个 id 或 null（未归属）。只改
   * `sessions.owner_workstream_id`，不碰工作路径与 Project —— 那两件事由后端保证，
   * 这里也不做任何补偿动作。
   */
  const setOwner = async (workstreamId: string | null) => {
    await api.setSessionOwnerWorkstream(sessionId, workstreamId);
    refresh();
    setOwnerOpen(false);
  };

  const messages: SessionMessageData[] = events.map((e) => ({
    sequence: e.sequence,
    kind: e.kind,
    text: e.text,
    ts: e.ts,
    who: agentDisplayLabel(session.agent),
    meta: e.metadata,
  }));

  return (
    <div className="main narrow">
      <PageHeader
        title={title}
        actions={trashed ? (
          // 回收站中的会话：摄入已停止、后端拒绝 Resume——两个入口都如实呈现为不可用。
          <button className="btn ghost icon-button" disabled
            aria-label="继续" title="回收站中的会话不能继续；先在上面的横幅里恢复它。">
            <Icon name="play" />
          </button>
        ) : (
          <>
            <button className="btn ghost icon-button" aria-label="移入回收站" title="移入回收站" onClick={() => setConfirmTrash(true)} disabled={trashBusy}>
              <Icon name="trash" />
            </button>
            <button className="btn ghost icon-button" aria-label="刷新" title="刷新" onClick={doSync} disabled={syncing}><Icon name="refresh" /></button>
            <button className="btn ghost icon-button" aria-label="继续" title="继续会话" onClick={() => setResumeOpen(true)}>
              <Icon name="play" />
            </button>
          </>
        )}
      />

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

      {/* §36.19：右栏承载只读事实（Agent、条数、会话信息），左栏是内容本身（所属任务、消息）。 */}
      <div className="task-detail-layout">
      <div className="task-detail-main">
      <div className="row between" style={{ marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>所属任务</div>
      </div>
      {/* 一次最多一个 Owner（方案 §3.3）：要么一条任务，要么「未归属任务」。
          没有「再添加一条」的入口——更换与清空都在这一个入口里。 */}
      {owner_workstream ? (
        <div className="rail-row">
          <div className="rail-main">
            <button
              className="link"
              style={{ textAlign: "left", overflowWrap: "anywhere" }}
              title={`打开任务：${owner_workstream.title}`}
              onClick={() => navigate({ view: "workstream", workstreamId: owner_workstream.id })}
            >
              {owner_workstream.title.trim() || "未命名任务"}
            </button>
          </div>
          <button className="btn small" onClick={() => setOwnerOpen(true)}>更改</button>
        </div>
      ) : (
        <div className="rail-row">
          <div className="rail-main">
            <div className="rail-title muted">未归属任务</div>
          </div>
          <button className="btn small" onClick={() => setOwnerOpen(true)}>选择</button>
        </div>
      )}

      <div className="section-label" style={{ marginTop: 34 }}>消息</div>
      <p className="muted small" style={{ margin: "0 0 6px" }}>
        {messages.length >= EVENT_PAGE_LIMIT
          ? `已显示 ${messages.length} 条消息，可能还有更早记录。`
          : null}
      </p>
      {messages.map((m) => <SessionMessage key={m.sequence} msg={m} />)}
      {messages.length === 0 && (
        <div className="empty">
          还没有摄入消息。
          <div className="small" style={{ marginTop: 4 }}>
            刷新以读取原始会话。
          </div>
          <div className="invite">
            <button className="btn small" onClick={doSync} disabled={syncing}>刷新</button>
          </div>
        </div>
      )}
      </div>

      <aside className="task-detail-aside">
      {/* 只读事实一律在右栏（§36.19），并且与另外两个详情页同形：rail-section + section-label，
          直接用展开的正文，不用 <details>——默认收起把这页最有用的事实藏了起来。 */}
      <section className="rail-section">
      <div className="section-label">会话信息</div>
      <div className="row" style={{ gap: 6 }}>
        <AgentIcon agent={session.agent} />
        <span>{agentDisplayLabel(session.agent)}</span>
      </div>
      <div className="row" style={{ gap: 8, marginTop: 6, flexWrap: "wrap" }}>
        {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
        {trashed && <span className="badge warn">回收站</span>}
        <span className="small muted">{messages.length > 0 ? `${messages.length} 条消息` : "尚无消息"}</span>
      </div>

      {/* 执行事实：一次会话「在什么时候、哪个目录、叫什么 ID」。值可整段选中并复制。 */}
      <div className="session-info-fields">
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
        <Field label="项目">
          {workspacePath ? (
            derivedProjectName ? (
              <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                <button
                  className="link"
                  title={`打开项目：${derivedProjectName}`}
                  onClick={() => navigate({ view: "project", projectId: workspacePath.project_id })}
                >
                  {derivedProjectName}
                </button>

              </span>
            ) : (
                  <span className="muted small">这条工作路径所属的项目记录暂时读不到。</span>
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
                ? "没有记录过工作目录，所以没有项目。"
                : "工作路径还没有登记，所以暂时没有项目。"}
            </span>
          )}
        </Field>
        <Field label="工作目录">
          {workspacePath ? (
            <>
              <CopyValue
                value={workspacePath.canonical_path}
                mono
                title={`${workspacePath.canonical_path} · NoEnding 识别工作位置用的规范化路径`}
              />
              {cwd !== "" && cwd !== workspacePath.canonical_path && (
                <div className="muted small" style={{ marginTop: 4 }}>
                  Agent 原始记录：{cwd}
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
            <>
              <CopyValue value={cwd} title={`${cwd} · Agent 原始记录里的 cwd，是这条会话自己的事实`} />
              <div className="muted small" style={{ marginTop: 4 }}>
                这个目录还没有被登记成工作路径 —— NoEnding 会在下一次目录扫描后自动补上，不需要手工操作。
              </div>
            </>
          ) : (
            <span className="muted">{NO_CWD}（该会话的原始记录里没有目录信息）</span>
          )}
        </Field>
        <Field label="原始会话文件">
          <>
            <CopyValue
              value={session.raw_path}
              mono
              title={`${session.raw_path} · Agent 保存的原始会话文件`}
            />
            {detail.raw_path_status === "missing" && (
              <div className="session-source-warning">找不到原始会话文件</div>
            )}
            {detail.raw_path_status === "unavailable" && (
              <div className="session-source-warning">无法确认原始会话文件状态</div>
            )}
          </>
        </Field>
        <Field label="会话 ID">
          <CopyValue value={session.id} mono />
        </Field>
        <Field label="Agent 侧会话 ID">
          <CopyValue value={session.agent_session_id} mono
            title="Agent 自己记录里的会话 ID，用于回到原始转录文件" />
        </Field>
        {/* §37.20 —— 同一次执行所在的父子会话树。父子链接是 Agent 侧的事实
            （Codex 线程、dsh 会话），在同一个 Agent 的 id 空间里解析。父会话可能
            不在库里（转录记了它，我们从没发现过那一条），那就如实说明；子会话
            按开始时间列出，回收站里的也在，用徽标标出。 */}
        {(detail.parent || session.parent_agent_session_id) && (
          <Field label="父会话">
            {detail.parent ? (
              <SessionLink session={detail.parent} navigate={navigate} />
            ) : (
              <span className="muted small">
                不在 NoEnding 库里
                <span className="mono" style={{ wordBreak: "break-all" }}>
                  {" · "}{session.parent_agent_session_id}
                </span>
              </span>
            )}
          </Field>
        )}
        {detail.children.length > 0 && (
          <Field label={`子会话（${detail.children.length}）`}>
            <div className="row" style={{ flexDirection: "column", alignItems: "stretch", gap: 4 }}>
              {detail.children.map((c) => (
                <SessionLink key={c.id} session={c} navigate={navigate} />
              ))}
            </div>
          </Field>
        )}
      </div>
      </section>
      </aside>
      </div>

      {/* 危险操作（§36）：只在正常状态下出现；回收站里的动作在顶部横幅。 */}
      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => { if (!trashBusy) setConfirmTrash(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会从 Sessions 列表、搜索与继续入口中消失，出现在 Sessions 页的「回收站」里。
          </p>
          {/* §36 要求把两个概念摆在同一处明确区分：「从任务移除」只改这条会话的
              所属任务（详情页的「更改 / 选择」），这里是全局回收站。 */}
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从任务移除</b> = 只修改这条会话的所属任务，会话本身留在列表里。
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
          onDeleted={() => { setPurgeOpen(false); goBack({ view: "sessions" }); }}
        />
      )}

      {ownerOpen && (
        <OwnerPickerModal
          currentOwnerId={owner_workstream?.id ?? null}
          projectId={sessionProjectId}
          onClose={() => setOwnerOpen(false)}
          onSubmit={setOwner}
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

/**
 * 父子会话行（§37.20）：点进那条会话的详情页。父子链接是 Agent 侧的事实，
 * 所以这里只负责导航——两条会话是不是同一个 Agent，后端解析时已经限定过。
 * 回收站里的孩子照样列出，用徽标如实标出，而不是藏起来。
 */
function SessionLink({ session, navigate }: {
  session: Session;
  navigate: (r: Route) => void;
}) {
  const title = sessionDisplayTitle(session.title);
  return (
    <button
      className="link"
      style={{ textAlign: "left", wordBreak: "break-word" }}
      title={`${title} · ${session.agent_session_id}`}
      onClick={() => navigate({ view: "session", sessionId: session.id })}
    >
      {title}
      {session.trashed_at !== null && (
        <span className="badge warn" style={{ marginLeft: 6 }}>回收站</span>
      )}
    </button>
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
 * 选择所属任务（方案 §29）：单选，一次只能选一个，也可以选「未归属」清空。
 *
 * 候选按「当前 Project → 其他任务」分组（§29.1）：与这条会话的工作目录有路径
 * 关联的任务优先出现。后端不建立这个限制，所以其他任务仍然可选中。
 * 归档的任务不接收新的归属（和列表里的候选同一套口径）。
 */
function OwnerPickerModal({ currentOwnerId, projectId, onClose, onSubmit }: {
  /** 当前所属任务；null = 未归属。 */
  currentOwnerId: string | null;
  /** null = 这条会话没有可解析的项目，此时不分组，只列全部任务。 */
  projectId: string | null;
  onClose: () => void;
  onSubmit: (workstreamId: string | null) => Promise<void>;
}) {
  const [tasks, setTasks] = useState<Workstream[] | null>(null);
  const [projectTaskIds, setProjectTaskIds] = useState<string[]>([]);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState<string | null>(currentOwnerId);
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const [failText, setFailText] = useState("");

  useEffect(() => {
    let active = true;
    setError(false);
    Promise.all([
      api.listWorkstreams(),
      projectId ? api.listWorkstreams(projectId) : Promise.resolve([]),
    ])
      .then(([all, inProject]) => {
        if (!active) return;
        setTasks(all.filter((w) => w.visibility === "normal"));
        setProjectTaskIds(inProject.map((w) => w.id));
      })
      .catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [projectId, attempt]);

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setFailText("");
    try {
      await onSubmit(selected);
    } catch (e) {
      setFailText(String(e));
    } finally {
      setBusy(false);
    }
  };

  const option = (id: string | null, title: string) => (
    <label className="existing-path-option" key={id ?? "none"}>
      <input
        type="radio"
        name="owner-workstream"
        checked={selected === id}
        onChange={() => setSelected(id)}
      />
      <span className="existing-path-info" title={title}>{title}</span>
    </label>
  );

  const otherTasks = (tasks ?? []).filter((w) => !projectTaskIds.includes(w.id));
  const projectTasks = (tasks ?? []).filter((w) => projectTaskIds.includes(w.id));

  return (
    <Modal title="选择所属任务" onClose={onClose}>
      <div className="existing-path-list" role="radiogroup" aria-label="所属任务">
        {error ? (
          <div role="alert">
            读取任务失败 <button className="btn small" onClick={() => setAttempt((n) => n + 1)}>重试</button>
          </div>
        ) : !tasks ? (
          <div role="status">加载中…</div>
        ) : (
          <>
            {option(null, "未归属")}
            {projectTasks.length > 0 ? (
              <>
                <div className="muted small" style={{ marginTop: 8 }}>当前项目</div>
                {projectTasks.map((w) => option(w.id, w.title.trim() || "未命名任务"))}
              </>
            ) : null}
            {projectTasks.length > 0 && otherTasks.length > 0 ? (
              <div className="muted small" style={{ marginTop: 8 }}>其他任务</div>
            ) : null}
            {otherTasks.map((w) => option(w.id, w.title.trim() || "未命名任务"))}
          </>
        )}
      </div>
      {failText && <div className="badge warn" style={{ marginTop: 8 }}>{failText}</div>}
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy || !tasks || selected === currentOwnerId} onClick={submit}>
          {busy ? "保存中…" : "保存"}
        </button>
      </div>
    </Modal>
  );
}
