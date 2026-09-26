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
  shrinkMiddle,
  NO_CWD,
  UNTITLED_SESSION,
} from "./SessionTable";
import {
  type Project,
  type SessionDetail,
  type SessionMemberRelation,
  type Workstream,
} from "../../types";
import type { Route } from "../../app/routes";

/** 详情里的一个执行成员：member 行 + 查询时带出的统计快照。 */
type DetailMember = SessionDetail["members"][number];

const MEMBER_ID_WIDTH = 24;

/**
 * Session Detail = 一个逻辑会话（重构方案 §4/§28）：一次用户可感知、可 Resume
 * 的主会话。§22 的四块内容——Conversation（只有 user/assistant prose）、
 * Execution Info（聚合统计 + 成员树）、Source / Lifecycle（源会话 + 回收站）、
 * Context / Owner（所属任务）。
 *
 * parent/children Session 链接已删除：执行图以 members 呈现，成员是执行信息，
 * 不是可进入的「另一个 Session 页面」（§22.2）。唯一的会话链接是 fork 来源
 * （§22.3）。右栏的 WorkspacePath / Project 事实照旧（v0.2 §43.3-M29）。
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
  // 执行成员树默认收起：统计常看，整棵图偶看（§22.2「可展开 Members」）。
  const [membersOpen, setMembersOpen] = useState(false);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字
   * （§43.4-2）。派生链自带名字，所以正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => { setDetail(d); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); });
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const needsProjectName = !detail?.workspace_path && !!detail?.session.project_id;
  useEffect(() => {
    if (!needsProjectName || projects) return;
    api.listProjects().then(setProjects).catch(console.error);
  }, [needsProjectName, projects]);
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
  const { session, owner_workstream } = detail;

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
      console.error(e);
      showToast(String(e));
    } finally {
      setTrashBusy(false);
    }
  };
  /** 派生链自带的路径与 Project（§43.3-M29：详情只承认这一种真相）。 */
  const workspacePath = detail.workspace_path;
  const projectIdOnlyCell = projectCellFor(session, projectNameById);
  const derivedProjectName = workspacePath && workspacePath.project_name.trim() !== ""
    ? workspacePath.project_name
    : null;
  /** 这条会话「属于」哪个项目：派生链优先，退回会话行上缓存的 project_id。 */
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

  /** Conversation（§22.1）：只有 root 的 user/assistant prose。 */
  const messages: SessionMessageData[] = detail.messages.map((m) => ({
    sequence: m.sequence,
    role: m.role,
    content: m.content,
    ts: m.ts,
    who: m.role === "user" ? "用户" : agentDisplayLabel(session.agent),
    provider: m.role === "assistant" ? m.provider : null,
    model: m.role === "assistant" ? m.model : null,
  }));

  /**
   * 源会话（§22.4）：Root 成员的源文件 + 详情加载时的新鲜结论。
   * members 里没有 root 行本身就是一个异常，按 unavailable 对待。
   */
  const rootMember = detail.members.find((m) => m.relation === "root") ?? null;
  const sourceMissing = rootMember !== null && detail.root_source_status === "missing";
  const sourceUnavailable = rootMember === null || detail.root_source_status === "unavailable";

  /** Resume 的门（§22.4）：源 missing / unavailable 时禁用，并说清为什么。 */
  const resumeDisabled = !detail.can_resume;
  const resumeTitle = detail.can_resume
    ? "继续会话"
    : sourceMissing
      ? "源会话已不存在，无法继续这个会话"
      : "无法确认源会话状态，暂时不能继续";

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
            <button className="btn ghost icon-button" aria-label="继续" title={resumeTitle} onClick={() => setResumeOpen(true)} disabled={resumeDisabled}>
              <Icon name="play" />
            </button>
          </>
        )}
      />

      {/* 回收站横幅（§36 + §22.4）：恢复永远可用；「永久删除」只在
          can_permanently_delete（trashed + fresh root missing）时出现。 */}
      {trashed && (
        <div className="session-trash-banner">
          <div style={{ minWidth: 0 }}>
            <b>该会话在回收站中</b>
            <div className="small muted" style={{ marginTop: 2 }}>
              移入回收站：{formatDateTime(session.trashed_at)}。NoEnding 已停止摄入这个会话；
              Agent 原始会话不会被删除，随时可以恢复。
            </div>
            {!detail.can_permanently_delete && (
              <div className="muted small" style={{ marginTop: 4 }}>
                永久删除不可用（Root 源仍存在或无法确认）。
              </div>
            )}
          </div>
          <div className="row" style={{ flex: "none", gap: 8 }}>
            <button className="btn small" disabled={trashBusy} onClick={doRestore}>恢复</button>
            {detail.can_permanently_delete && (
              <button className="btn small" disabled={trashBusy} onClick={() => setPurgeOpen(true)}>
                永久删除…
              </button>
            )}
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

      {/* 执行事实：一次会话「在什么时候、哪个目录、源在哪里」。值可整段选中并复制。 */}
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
              {projectIdOnlyCell.text}
              {" —— "}
              {projectIdOnlyCell.hint}
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
        {/* 源会话（§22.4）：Root 成员的源文件，不再是 session.raw_path。
            状态是详情加载时对源的新鲜结论，missing / unavailable 都如实说出。 */}
        <Field label="源会话">
          {rootMember ? (
            <>
              <CopyValue
                value={rootMember.source_path}
                mono
                title={`${rootMember.source_path} · Agent 保存的 Root 源会话`}
              />
              {sourceMissing && (
                <div className="session-source-warning">源会话已不存在</div>
              )}
              {sourceUnavailable && (
                <div className="session-source-warning">无法确认源会话状态</div>
              )}
            </>
          ) : (
            <>
              <span className="muted small">这个会话没有 Root 成员记录。</span>
              <div className="session-source-warning">无法确认源会话状态</div>
            </>
          )}
        </Field>
        <Field label="会话 ID">
          <CopyValue value={session.id} mono />
        </Field>
        <Field label="Agent 会话 ID">
          <CopyValue value={session.root_agent_session_id} mono
            title="Root 成员在 Agent 侧的会话身份；Resume 与 LaunchIntent 匹配的唯一依据" />
        </Field>
        {/* Fork（§22.3）：唯一的会话→会话链接。来源只是 provenance，
            生命周期完全独立；来源不在本地库时也如实说明。 */}
        {(detail.forked_from || session.forked_from_session_id) && (
          <Field label="来源">
            {detail.forked_from ? (
              <button
                className="link"
                style={{ textAlign: "left", wordBreak: "break-word" }}
                title={`打开来源会话：${sessionDisplayTitle(detail.forked_from.title)}`}
                onClick={() => navigate({ view: "session", sessionId: detail.forked_from!.id })}
              >
                分叉自：{sessionDisplayTitle(detail.forked_from.title)}
                {detail.forked_from.trashed_at !== null && (
                  <span className="badge warn" style={{ marginLeft: 6 }}>回收站</span>
                )}
              </button>
            ) : (
              <span className="muted small">
                来源会话不在 NoEnding 库里
                <span className="mono" style={{ wordBreak: "break-all" }}>
                  {" · "}{session.forked_from_session_id}
                </span>
              </span>
            )}
          </Field>
        )}
      </div>
      </section>

      {/* 执行信息（§22.2）：聚合统计 + 可展开的成员树。成员不是链接——
          它们是 Agent 内部的执行单元，不是另一个 Session 页面。 */}
      <section className="rail-section">
      <div className="section-label">执行信息</div>
      <ExecutionStats stats={detail.stats} />
      {detail.members.length > 0 && (
        <>
          <div style={{ marginTop: 8 }}>
            <button
              className="btn small ghost"
              aria-expanded={membersOpen}
              onClick={() => setMembersOpen((o) => !o)}
            >
              {membersOpen
                ? "收起成员"
                : `展开成员（${detail.members.length}）`}
            </button>
          </div>
          {membersOpen && <MemberTree members={detail.members} />}
        </>
      )}
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

/** 数值 → 千位分隔；null = 源不提供，显示「—」。 */
const fmtCount = (n: number | null): string => (n === null ? "—" : n.toLocaleString("en-US"));

/**
 * 聚合执行统计（§22.2）。基础形状（成员 / 子 / 边 / 深度）永远显示；
 * 工具、token、成本、模型这些行只在数据真的在场上时出现——有数据才显示，
 * 没有就不画一行「0」去冒称观测。
 */
function ExecutionStats({ stats }: { stats: SessionDetail["stats"] }) {
  const toolBits: string[] = [];
  if (stats.tool_call_count > 0) toolBits.push(`工具调用 ${stats.tool_call_count}`);
  if (stats.tool_error_count > 0) toolBits.push(`失败 ${stats.tool_error_count}`);
  if (stats.compaction_count > 0) toolBits.push(`压缩 ${stats.compaction_count}`);
  if (stats.side_activity_count > 0) toolBits.push(`边活动 ${stats.side_activity_count}`);

  const tokenBits: string[] = [];
  if (stats.input_tokens !== null) tokenBits.push(`输入 ${fmtCount(stats.input_tokens)}`);
  if (stats.output_tokens !== null) tokenBits.push(`输出 ${fmtCount(stats.output_tokens)}`);
  if (stats.cached_tokens !== null) tokenBits.push(`缓存 ${fmtCount(stats.cached_tokens)}`);
  if (stats.reasoning_tokens !== null) tokenBits.push(`推理 ${fmtCount(stats.reasoning_tokens)}`);

  // 不再显示"Session 的 model/provider/effort"（Provenance 方案 §9）：一个
  // Logical Session 完全可能中途切模型，不存在天然的 Session model。消息级
  // 的模型标签在会话消息上；将来要按模型统计时从 assistant 消息派生。

  return (
    <div style={{ display: "grid", gap: 4, marginTop: 8 }}>
      <div className="small">
        成员 {stats.member_count} · 子 {stats.child_count} · 边 {stats.side_count} · 最大深度 {stats.max_depth}
      </div>
      {toolBits.length > 0 && <div className="small muted">{toolBits.join(" · ")}</div>}
      {tokenBits.length > 0 && <div className="small muted">Tokens {tokenBits.join(" · ")}</div>}
      {stats.cost !== null && (
        <div className="small muted">成本 {stats.cost.toLocaleString("en-US", { maximumFractionDigits: 4 })}</div>
      )}
    </div>
  );
}

const RELATION_LABELS: Record<SessionMemberRelation, string> = {
  root: "根",
  child: "子",
  side: "边执行",
};

/** 一行成员：关系标签 + 稳定身份（mono，截断，原值在 title 里）+ 自身计数。 */
function MemberRow({ member, depth }: { member: DetailMember; depth: number }) {
  const s = member.stats;
  const bits: string[] = [];
  if (s?.tool_call_count != null) bits.push(`工具 ${s.tool_call_count}`);
  if (s?.tool_error_count != null && s.tool_error_count > 0) bits.push(`失败 ${s.tool_error_count}`);
  if (s?.compaction_count != null && s.compaction_count > 0) bits.push(`压缩 ${s.compaction_count}`);

  return (
    <div className="row" style={{ paddingLeft: depth * 18, gap: 8, minWidth: 0, alignItems: "baseline" }}>
      <span className="badge" style={{ flex: "none" }}>{RELATION_LABELS[member.relation]}</span>
      <span
        className="mono small"
        style={{ minWidth: 0, overflowWrap: "anywhere" }}
        title={member.source_member_id}
      >
        {shrinkMiddle(member.source_member_id, MEMBER_ID_WIDTH)}
      </span>
      {bits.length > 0 && <span className="muted small" style={{ flex: "none" }}>{bits.join(" · ")}</span>}
    </div>
  );
}

/**
 * 成员树（§22.2）：root 在顶，child / side 挂在自己的 parent 下，缩进呈现。
 * parent 记录缺席（未摄入或被清理）的成员不能消失——按顶层孤儿如实列出。
 */
function MemberTree({ members }: { members: DetailMember[] }) {
  const byParent = useMemo(() => {
    const map = new Map<string, DetailMember[]>();
    const ids = new Set(members.map((m) => m.source_member_id));
    const top: DetailMember[] = [];
    for (const m of members) {
      const parent = m.relation === "root" ? null : m.parent_source_member_id;
      if (parent === null || !ids.has(parent)) top.push(m);
      else {
        const list = map.get(parent) ?? [];
        list.push(m);
        map.set(parent, list);
      }
    }
    return { map, top };
  }, [members]);

  const renderNode = (m: DetailMember, depth: number): React.ReactNode[] => [
    <MemberRow key={m.id} member={m} depth={depth} />,
    ...(byParent.map.get(m.source_member_id) ?? []).flatMap((c) => renderNode(c, depth + 1)),
  ];

  return (
    <div style={{ display: "grid", gap: 6, marginTop: 10 }}>
      {byParent.top.flatMap((m) => renderNode(m, 0))}
    </div>
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
