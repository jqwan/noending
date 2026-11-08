import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { notifyDataChanged, type Route } from "../../app/routes";
import { Field, openPath, timeAgo } from "../../components/common";
import { DetailFields, DetailSection } from "../../components/DetailSection";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import type { Project, SessionDetail } from "../../types";
import { agentDisplayLabel, formatDateTime, projectCellFor, sessionDisplayTitle, NO_CWD, UNTITLED_SESSION } from "./SessionTable";
import SessionTrackedTasks from "./SessionTrackedTasks";
import SessionTaskTrackingModal from "./SessionTaskTrackingModal";

/** 会话详情与历史消息共用信息、跟踪任务及其操作。 */
export default function SessionDetailPanel({ detail, navigate }: {
  detail: SessionDetail;
  navigate: (route: Route) => void;
}) {
  const { session } = detail;
  const sessionId = session.id;
  const [trackingBusy, setTrackingBusy] = useState(false);
  const [trackingError, setTrackingError] = useState("");
  const trackingPending = useRef(false);
  const trackingTarget = useRef<string | null>(sessionId);
  useEffect(() => {
    trackingTarget.current = sessionId; setTrackingError(""); setTrackingBusy(false);
    return () => { trackingTarget.current = null; };
  }, [sessionId]);
  /**
   * 只有在「缓存列里有 Project、却没有任何工作路径可解析」时才需要名字；
   * 派生链自带名字，正常情况下不多这一次读取。
   */
  const [projects, setProjects] = useState<Project[] | null>(null);

  const needsProjectName = !detail?.workspace_path && !!detail?.session.project_id;
  useEffect(() => {
    if (!needsProjectName || projects) return;
    api.listProjects().then(setProjects).catch(console.error);
  }, [needsProjectName, projects]);
  const projectNameById = useMemo(
    () => new Map((projects ?? []).map((p) => [p.id, p.name])),
    [projects],
  );

  const [trackModalOpen, setTrackModalOpen] = useState(false);
  const title = sessionDisplayTitle(session.title);
  const untitled = title === UNTITLED_SESSION;
  const cwd = (session.cwd ?? "").trim();
  const archived = session.archived_at !== null;
  const workspacePath = detail.workspace_path;
  const projectIdOnlyCell = projectCellFor(session, projectNameById);
  const derivedProjectName = workspacePath && workspacePath.project_name.trim() !== ""
    ? workspacePath.project_name : null;
  const removeTracking = async (taskId: string) => {
    if (trackingPending.current) return;
    trackingPending.current = true; setTrackingBusy(true); setTrackingError("");
    try {
      await api.setSessionTaskTracking(sessionId, taskId, false);
      notifyDataChanged([]);
    } catch (reason) {
      if (trackingTarget.current === sessionId) setTrackingError(String(reason));
    } finally {
      trackingPending.current = false;
      if (trackingTarget.current === sessionId) setTrackingBusy(false);
    }
  };
  /** 源会话：会话自己的源文件 + 详情加载时的新鲜结论。 */
  const sourceMissing = detail.source_status === "missing";
  const sourceUnavailable = detail.source_status === "unavailable";
  /** 源在，路径本身才是入口；源不在就只是可复制的文本，点了也只会报错。 */
  const canRevealSource = !sourceMissing && !sourceUnavailable;

  const revealSource = async () => {
    try {
      await api.revealSessionSource(sessionId);
    } catch (e) {
      showToast(`定位失败：${String(e)}`);
    }
  };

  return <>
<DetailSection sectionId="properties" label="会话信息" title={<>
    <Icon name="info" />
    <span>会话信息</span>
  </>}>

  {(untitled || archived) && (
    <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      {untitled && <span className="badge" title="原始转录里没有可用的标题">无标题</span>}
      {archived && <span className="badge warn">已归档</span>}
    </div>
  )}

  {/* 执行事实：一次会话「在什么时候、哪个目录、源在哪里」。值可整段选中并复制。 */}
  <DetailFields>
    <Field label="Agent">
      <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
        <button
          type="button"
          className="link"
          title={`打开 Agent：${agentDisplayLabel(session.agent)}`}
          onClick={() => navigate({ view: "agents", agent: session.agent })}
        >
          <span>{agentDisplayLabel(session.agent)}</span>
        </button>
      </span>
    </Field>
    <Field label="项目">
      {workspacePath ? (
        derivedProjectName ? (
          <span className="row" style={{ gap: 8, flexWrap: "wrap" }}>
            <button
              className="link"
              title={`打开项目：${derivedProjectName}`}
              onClick={() => navigate({ view: "project", projectId: workspacePath.project_id, initialTitle: derivedProjectName })}
            >
              <span>{derivedProjectName}</span>
            </button>
          </span>
        ) : (
          <span className="muted small">项目信息暂不可用</span>
        )
      ) : session.project_id ? (
        <span className="muted small" style={{ wordBreak: "break-word" }}>
          {projectIdOnlyCell.text}
          {" —— "}
          {projectIdOnlyCell.hint}
        </span>
      ) : (
        <span className="muted small">无关联项目</span>
      )}
    </Field>
    <Field
      label="工作目录"
      copyValue={workspacePath ? workspacePath.canonical_path : (cwd || undefined)}
      copyTitle="复制工作目录"
    >
      {workspacePath ? (
        <>
          <button
            type="button"
            className="link mono"
            title={workspacePath.canonical_path}
            onClick={() => void openPath(workspacePath.canonical_path)}
          >
            <span>{workspacePath.canonical_path}</span>
          </button>
          {cwd !== "" && cwd !== workspacePath.canonical_path && (
            <div className="muted small" style={{ marginTop: 4 }}>
              Agent 原始记录：
              <button
                type="button"
                className="link mono"
                title={cwd}
                onClick={() => void openPath(cwd)}
              >
                <span>{cwd}</span>
              </button>
            </div>
          )}
          {!workspacePath.exists && (
            <div style={{ marginTop: 4 }}>
              <span
                className="badge warn"
                title="磁盘中未找到该目录"
              >
                目录不存在
              </span>
            </div>
          )}
        </>
      ) : cwd ? (
        <>
          <button
            type="button"
            className="link mono"
            title={cwd}
            onClick={() => void openPath(cwd)}
          >
            <span>{cwd}</span>
          </button>
          <div className="muted small" style={{ marginTop: 4 }}>
            未登记为工作路径
          </div>
        </>
      ) : (
        <span className="muted">{NO_CWD}（无目录记录）</span>
      )}
    </Field>
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
    {archived && session.archived_at && (
      <Field label="归档时间">
        <span title={session.archived_at}>{formatDateTime(session.archived_at)}</span>
      </Field>
    )}
    <Field label="会话统计">
      <span className="row" style={{ gap: 12, flexWrap: "wrap" }}>
        <span>用户消息 {detail.message_stats?.user_messages ?? detail.messages.filter((m) => m.role === "user").length}</span>
        <span>Agent 回复 {detail.message_stats?.assistant_messages ?? detail.messages.filter((m) => m.role === "assistant").length}</span>
      </span>
    </Field>
    {/* 源会话：会话自己的源文件。
        状态是详情加载时对源的新鲜结论，missing / unavailable 都如实说出。
        路径本身就是「在文件管理器中显示」的入口，不再另配一行链接。 */}
    <Field
      label="源会话"
      copyValue={session.source_path}
      copyTitle="复制源会话路径"
    >
      {canRevealSource ? (
        <button
          className="link mono"
          title={session.source_path}
          style={{ minWidth: 0, wordBreak: "break-all", textAlign: "left" }}
          onClick={() => void revealSource()}
        >
          {session.source_path}
        </button>
      ) : (
        <span
          className="mono"
          title={session.source_path}
          style={{ minWidth: 0, wordBreak: "break-all", userSelect: "all" }}
        >
          {session.source_path}
        </span>
      )}
      {session.agent === "codex" && session.source_path.endsWith(".jsonl.zst") && (
        <span className="badge muted" title="读取时自动解压，不修改源文件">Zstd 压缩</span>
      )}
      {sourceMissing && (
        <div className="session-source-warning">源会话已不存在</div>
      )}
      {sourceUnavailable && (
        <div className="session-source-warning">无法确认源会话状态</div>
      )}
    </Field>
    <Field label="会话 ID" copyValue={`NoEnding ID：${session.id}`} copyTitle="复制会话 ID">
      <div className="mono" style={{ wordBreak: "break-all", userSelect: "all" }}>
        {session.id}
      </div>
    </Field>
    <Field
      label="Agent 会话 ID"
      copyValue={session.root_agent_session_id}
      copyTitle="复制 Agent 会话 ID"
    >
      <div
        className="mono"
        title="原始会话 ID"
        style={{ wordBreak: "break-all", userSelect: "all" }}
      >
        {session.root_agent_session_id}
      </div>
    </Field>
    {/* Fork：唯一的会话→会话链接。来源只是 provenance，
        生命周期完全独立；来源不在本地库时也如实说明。 */}
    {(detail.forked_from || session.forked_from_session_id) && (
      <Field label="来源">
        {detail.forked_from ? (
          <button
            className="link"
            style={{ textAlign: "left", wordBreak: "break-word" }}
            title={`打开来源会话：${sessionDisplayTitle(detail.forked_from.title)}`}
            onClick={() => navigate({ view: "session", sessionId: detail.forked_from!.id, initialTitle: sessionDisplayTitle(detail.forked_from!.title), initialAgent: detail.forked_from!.agent })}
          >
            分叉自：{sessionDisplayTitle(detail.forked_from.title)}
            {detail.forked_from.archived_at !== null && (
              <span className="badge warn" style={{ marginLeft: 6 }}>已归档</span>
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
  </DetailFields>
</DetailSection>
<SessionTrackedTasks tasks={detail.tracked_workstreams} navigate={navigate}
  busy={trackingBusy} error={trackingError} onAdd={() => setTrackModalOpen(true)}
  onRemove={id => void removeTracking(id)} />
      {trackModalOpen && (
        <SessionTaskTrackingModal
          sessionId={sessionId}
          onClose={() => setTrackModalOpen(false)}
        />
      )}
  </>;
}
