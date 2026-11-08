import { DetailPanelProvider } from "../../components/DetailLayout";
import { DetailContent, DetailFields, DetailList, DetailListRow, DetailSection } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import { removedTargets } from "../../app/removedTargets";
import { useCallback, useEffect, useRef, useState } from "react";
import "./workstreams.css";
import { api } from "../../api";

import PageHeader from "../../layout/PageHeader";
import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal, Modal, Field } from "../../components/common";
import WorkstreamFormModal from "./WorkstreamFormModal";
import WorkstreamProjectPickerModal from "./WorkstreamProjectPickerModal";
import {
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
} from "../../types";
import type { Route, WorkstreamEntry } from "../../app/routes";
import { notifyDataChanged } from "../../app/routes";
import CollaborationSummary from "../collaboration/CollaborationSummary";
import WorkstreamSessionPickerModal from "./WorkstreamSessionPickerModal";
import TaskItems from "./TaskItems";
import TaskSubpageTabs from "./TaskSubpageTabs";
import CollaborationRoomView from "../collaboration/CollaborationRoomView";

/** Task organization. All background reads are pure. */
import { workstreamDetailCache, clearWorkstreamDetailCache } from "./workstreamDetailCache";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
import { collaborationRoomResource } from "../collaboration/collaborationResources";
export { workstreamDetailCache, clearWorkstreamDetailCache };


export default function WorkstreamDetailView({
  workstreamId,
  initialTitle,
  entry,
  navigate,
  goBack,
}: {
  workstreamId: string;
  initialTitle?: string;
  entry?: WorkstreamEntry;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const currentWorkstreamId = useRef(workstreamId);
  currentWorkstreamId.current = workstreamId;
  const cached = workstreamDetailCache.get(workstreamId);
  const [loadError, setLoadError] = useState("");
  const [retry, setRetry] = useState(0);
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(() => cached?.ctx ?? null);
  const showSkeleton = useLoadingDelay(!ctx && loadError === "");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);
  const [projectPickerOpen, setProjectPickerOpen] = useState(false);
  const [sessionPickerOpen, setSessionPickerOpen] = useState(false);
  const collaboration = collaborationRoomResource.useValue(workstreamId);
  const lockedSessionIds = collaboration?.run?.status === "active" || collaboration?.run?.status === "starting"
    ? collaboration.members.map(member => member.session_id) : [];
  const [actionError, setActionError] = useState("");
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const busyRef = useRef(false);



  useEffect(() => {
    let cancelled = false;
    const currentCached = workstreamDetailCache.get(workstreamId);
    if (currentCached) {
      setCtx(currentCached.ctx);
    } else {
      setCtx(null);
    }
    setLoadError("");
    setEditing(false);
    setConfirmArchive(false);
    setConfirmPurge(false);
    setActionError("");

    api.getWorkstreamContext(workstreamId).then((c) => {
      if (cancelled || removedTargets.workstreams.has(workstreamId)) return;
      setCtx(c);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
      });
    }).catch(e => { if (!cancelled) setLoadError(String(e)); });


    return () => {
      cancelled = true;
    };
  }, [workstreamId, retry]);

  // Background refresh only reads task data.
  const refresh = useCallback(() => {
    return api.getWorkstreamContext(workstreamId).then((c) => {
      if (currentWorkstreamId.current !== workstreamId || removedTargets.workstreams.has(workstreamId)) return;
      setCtx(c);
      workstreamDetailCache.set(workstreamId, {
        ctx: c,
      });
    }).catch(console.error);



  }, [workstreamId]);

  useRefreshSignal(refresh);


  // 同 SessionDetailView：加载态也要渲染 PageHeader，否则整条标题栏会先消失再补回来。
  if (!ctx) {
    if (loadError === "") {
      return (
        <div className="main task-detail" role="status">
          <PageHeader
            title={initialTitle ?? ""} icon={<Icon name="tasks" />}
            navigation={<TaskSubpageTabs workstreamId={workstreamId} entry={entry} initialTitle={initialTitle} navigate={navigate} />}
          />
          {showSkeleton && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
              <div className="skeleton" style={{ height: 120, borderRadius: "var(--radius-lg)" }} />
              <div className="skeleton" style={{ height: 200, borderRadius: "var(--radius-lg)" }} />
            </div>
          )}
        </div>
      );
    }
    return (
      <div className="main narrow" role="status">
        <PageHeader title={initialTitle ?? ""} icon={<Icon name="tasks" />}>
          <p>{loadError}</p>
          <button className="btn" onClick={() => setRetry(value => value + 1)}>重试</button>
        </PageHeader>
      </div>
    );
  }
  const { workstream, sessions } = ctx;

  /** 一条命令返回新 Workstream 时立刻就地替换：`update_workstream` 是整对象写，
   *  留着旧的 visibility，下一次整对象保存会被后端拒绝。 */
  const adopt = (next: Workstream) => setCtx((c) => {
    const nextCtx = c ? { ...c, workstream: next } : c;
    if (nextCtx) {
      const prev = workstreamDetailCache.get(workstreamId);
      if (prev) {
        workstreamDetailCache.set(workstreamId, { ...prev, ctx: nextCtx });
      }
    }
    return nextCtx;
  });

  /**
   * 归档与恢复是两个单向命令：`archive_workstream` 只进已归档，
   * `restore_workstream` 只出来——不是一枚翻转开关（那会造出双权威）。
   * 失败留在原地说明原因，成功才跳走。
   */
  const archiveTask = async () => {
    setConfirmArchive(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.archiveWorkstream(workstream.id));
      goBack({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`归档失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const restore = async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.restoreWorkstream(workstream.id));
      refresh();
    } catch (e) {
      console.error(e);
      setActionError(`恢复失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /** 唯一不可逆的动作，且后端只接受从已归档出发。 */
  const purge = async () => {
    setConfirmPurge(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      await api.deleteWorkstreamPermanently(workstream.id);
      workstreamDetailCache.delete(workstream.id);
      if (mounted.current) navigate({ view: "workstreams", scope: "archived" });
    } catch (e) {
      console.error(e);
      setActionError(`永久删除失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const openEditor = () => {
    setEditing(true);
  };

  const archived = workstream.visibility === "archived";
  // 一个 Workstream 可以关联多个 Project
  const projectRows=ctx.projects;

  return (
    <DetailPanelProvider kind={entry === "collaboration" ? "collaboration" : "task"}><div className="main detail-page task-detail">
      <PageHeader
        title={workstream.title} icon={<Icon name="tasks" />}
        navigation={<TaskSubpageTabs
              workstreamId={workstream.id}
              entry={entry}
              initialTitle={workstream.title}
              navigate={navigate}
            />}
        actions={
          <>
            <button className="btn ghost icon-button" title="编辑任务" aria-label="编辑任务"
              onClick={openEditor}>
              <Icon name="edit" />
            </button>
            {!archived ? (
              <button className="btn ghost icon-button" aria-label="归档"
                title="归档：保留项目关联、会话跟踪与摘要，归档后不能新建会话。"
                onClick={() => setConfirmArchive(true)}>
                <Icon name="archive" />
              </button>
            ) : (
              <>
                <button
                  className="btn ghost icon-button"
                  title="取消归档"
                  aria-label="取消归档"
                  disabled={busy}
                  onClick={restore}
                >
                  <Icon name="unarchive" />
                </button>
                <button
                  className="btn ghost icon-button danger"
                  title="永久删除"
                  aria-label="永久删除"
                  disabled={busy}
                  onClick={() => setConfirmPurge(true)}
                >
                  <Icon name="trash" />
                </button>
              </>
            )}
          </>
        }
      >
        {actionError && (
          <div className="small" style={{ color: "var(--warning)", marginTop: 6, overflowWrap: "anywhere" }}>
            {actionError}
          </div>
        )}
      </PageHeader>

      {/* 详情与协作消息共用右栏及成员选择，只切换主内容。 */}
        <CollaborationRoomView
          taskId={workstream.id}
          taskTitle={workstream.title}
          sessions={sessions}
          archived={archived}
          navigate={navigate}
          showChat={entry === "collaboration"}
          onManageSessions={archived || (!projectRows.length && !sessions.length) ? undefined : () => setSessionPickerOpen(true)}
          detailsPanel={<><DetailSection sectionId="properties" label="任务信息" title={<>
              <Icon name="info" />
              <span>任务信息</span>
            </>}>

            <DetailFields>
            <Field label="状态"><span className="badge">{archived ? "已归档" : "未归档"}</span></Field>
            <Field label="创建时间">{formatDate(workstream.created_at)}</Field>
            <Field label="最近更新">{timeAgo(workstream.updated_at)}</Field>
            <Field label="任务 ID" copyValue={workstream.id} copyTitle="复制任务 ID">
              <div className="mono" style={{ wordBreak: "break-all", userSelect: "all" }}>
                {workstream.id}
              </div>
            </Field>
            </DetailFields>
          </DetailSection>
<DetailSection sectionId="projects" label="关联项目" title={<>
                <Icon name="project" />
                <span>关联项目（{projectRows.length}）</span>
              </>} actions={<>{!archived && (
                <button
                  type="button"
                  className="btn ghost icon-button small"
                  title="关联项目"
                  aria-label="关联项目"
                  onClick={() => setProjectPickerOpen(true)}
                >
                  <Icon name="plus" />
                </button>
              )}</>}>

            {projectRows.length === 0 && (
              <div className="detail-empty">
                暂无项目
              </div>
            )}
            <DetailList label="关联项目列表">
              {projectRows.map(p => <DetailListRow key={p.id} icon={<Icon name="project" />}
                title={p.name ?? "未命名项目"} titleHint={p.name ?? p.id}
                onOpen={() => navigate({ view: "project", projectId: p.id, initialTitle: p.name ?? undefined })}
              />)}
            </DetailList>
          </DetailSection></>}>
          <DetailSection sectionId="description" label="任务描述" title={<>
                <Icon name="tasks" />
                <span>任务描述</span>
              </>}>

            {workstream.description ? (
              <DetailContent label="任务描述内容"><p className="detail-description">
                {workstream.description}
              </p></DetailContent>
            ) : (
              <div className="detail-empty">还没有描述。</div>
            )}
          </DetailSection>
<TaskItems key={workstream.id} taskId={workstream.id} />
<CollaborationSummary key={`summary-${workstream.id}`} taskId={workstream.id} />
        </CollaborationRoomView>

      {sessionPickerOpen && <WorkstreamSessionPickerModal projectIds={projectRows.map(project => project.id)} linkedSessions={sessions} lockedIds={lockedSessionIds}
        onClose={() => setSessionPickerOpen(false)} onConfirm={async changes => {
          try { for (const change of changes) await api.setSessionTaskTracking(change.sessionId, workstream.id, change.tracked); }
          finally {
            await Promise.all([refresh(), collaborationRoomResource.refresh(workstream.id).catch(() => {})]);
            notifyDataChanged([]);
          }
        }} />}

      {projectPickerOpen && (
        <WorkstreamProjectPickerModal
          initialSelectedIds={projectRows.map((p) => p.id)}
          onClose={() => setProjectPickerOpen(false)}
          onConfirm={async (nextSelected) => {
            await api.setWorkstreamProjects(workstream.id, nextSelected);
            refresh();
          }}
        />
      )}

      {editing && (
        <WorkstreamFormModal
          workstream={workstream}
          projects={projectRows}
          onClose={() => setEditing(false)}
          onSaved={refresh}
        />
      )}

      {confirmArchive && (
        <Modal title="归档" onClose={() => setConfirmArchive(false)}>
          <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
            <b>{workstream.title}</b> 会移至任务页的「已归档」，归档后不能用于新建会话。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            路径、会话归属和上下文都会保留，可取消归档。
          </p>

          <DialogActions>
            <button className="btn" onClick={() => setConfirmArchive(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={archiveTask} disabled={busy}>
              {busy ? "处理中…" : "归档"}
            </button>
          </DialogActions>
        </Modal>
      )}

      {confirmPurge && (
        <Modal title="永久删除这项任务？" onClose={() => { if (!busy) setConfirmPurge(false); }}>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>不可撤销</div>
          <p style={{ margin: "0 0 12px", maxWidth: "72ch" }}>
            确定要永久删除 <b>{workstream.title}</b> 吗？任务本身、工作路径列表、Context 和审阅数据会被删除，关联会话及其事件历史会保留。
          </p>
          <DialogActions>
            <button className="btn" onClick={() => setConfirmPurge(false)} disabled={busy}>取消</button>
            <button className="btn danger" onClick={purge} disabled={busy}>
              {busy ? "删除中…" : "永久删除"}
            </button>
          </DialogActions>
        </Modal>
      )}
    </div></DetailPanelProvider>
  );
}

/**
 * RFC3339 (UTC) → 本地 YYYY/MM/DD。不用 `toLocaleDateString()`（webview 语言不固定，
 * 格式会漂移），也不用 `slice(0, 10)`（那是 UTC 日期，本地可能已跨天）。
 */
function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getFullYear()}/${pad(t.getMonth() + 1)}/${pad(t.getDate())}`;
}
