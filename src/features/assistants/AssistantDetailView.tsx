import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { notifyDataChanged, type Route } from "../../app/routes";
import { Field, Modal, timeAgo } from "../../components/common";
import CompactPath from "../../components/CompactPath";
import { DetailLayout, DetailPanelProvider } from "../../components/DetailLayout";
import { DetailFields, DetailList, DetailListRow, DetailSection } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import Icon, { type IconName } from "../../components/Icon";
import Notice from "../../components/Notice";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
import PageHeader from "../../layout/PageHeader";
import type {
  AssistantDetail,
  Project,
  SessionListItem,
  Workstream,
} from "../../types";
import SkillScopeConfig from "../skills/SkillScopeConfig";
import {
  assistantDetailCache,
  clearAssistantCardsCache,
} from "./assistantsCache";
import EditProfileModal from "./EditProfileModal";
import "./assistants.css";

export default function AssistantDetailView({
  assistantId,
  initialTitle,
  navigate,
  goBack,
}: {
  assistantId: string;
  initialTitle?: string;
  entry?: "detail" | "sessions";
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
}) {
  const [data, setData] = useState<AssistantDetail | null>(
    () => assistantDetailCache.get(assistantId) ?? null,
  );
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(data === null);
  const showSkeleton = useLoadingDelay(loading && data === null);

  // Instructions editor state
  const [instructionsInput, setInstructionsInput] = useState("");
  const [instructionsModified, setInstructionsModified] = useState(false);
  const instructionsModifiedRef = useRef(false);
  instructionsModifiedRef.current = instructionsModified;
  const draftBaseRevision = useRef<number | null>(null);
  const [draftConflictNotice, setDraftConflictNotice] = useState("");
  const [savingInstructions, setSavingInstructions] = useState(false);
  const [syncingFiles, setSyncingFiles] = useState(false);

  // Edit profile modal
  const [editModalOpen, setEditModalOpen] = useState(false);

  // Sessions list
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [loadingSessions, setLoadingSessions] = useState(false);
  const [nextSessionCursor, setNextSessionCursor] = useState<string | null>(null);
  const [hasMoreSessions, setHasMoreSessions] = useState(false);
  const [loadingMoreSessions, setLoadingMoreSessions] = useState(false);

  // Responsibilities modal
  const [respModalOpen, setRespModalOpen] = useState(false);
  const [availableProjects, setAvailableProjects] = useState<Project[]>([]);
  const [availableTasks, setAvailableTasks] = useState<Workstream[]>([]);
  const [selectedProjectIds, setSelectedProjectIds] = useState<string[]>([]);
  const [selectedTaskIds, setSelectedTaskIds] = useState<string[]>([]);
  const [savingResp, setSavingResp] = useState(false);

  // Archive modal
  const [archiveModalOpen, setArchiveModalOpen] = useState(false);
  const [archiveBusy, setArchiveBusy] = useState(false);

  // Starting session
  const [startingSession, setStartingSession] = useState(false);

  const requestSeq = useRef(0);

  const loadDetail = useCallback(async () => {
    const seq = ++requestSeq.current;
    setLoading(true);
    setFailure("");
    try {
      const detail = await api.getAssistantProfile(assistantId);
      if (seq === requestSeq.current) {
        setData(detail);
        assistantDetailCache.set(assistantId, detail);
        if (!instructionsModifiedRef.current) {
          setInstructionsInput(detail.profile.instructions);
          setInstructionsModified(false);
          draftBaseRevision.current = detail.profile.revision;
          setDraftConflictNotice("");
        } else if (
          draftBaseRevision.current !== null &&
          detail.profile.revision !== draftBaseRevision.current
        ) {
          setDraftConflictNotice(
            `后台档案已更新为版本 r${detail.profile.revision}，当前草稿基于版本 r${draftBaseRevision.current}。请选择覆盖远端或放弃草稿以重载。`,
          );
        }
      }
    } catch (err) {
      if (seq === requestSeq.current) {
        setFailure(String(err));
      }
    } finally {
      if (seq === requestSeq.current) {
        setLoading(false);
      }
    }
  }, [assistantId]);

  const loadSessions = useCallback(async () => {
    setLoadingSessions(true);
    try {
      const res = await api.listAssistantSessions(assistantId, { limit: 20 });
      setSessions(res.items);
      setNextSessionCursor(res.next_cursor ?? null);
      setHasMoreSessions(!res.end && Boolean(res.next_cursor));
    } catch {
      // 容错处理
    } finally {
      setLoadingSessions(false);
    }
  }, [assistantId]);

  const handleLoadMoreSessions = async () => {
    if (!nextSessionCursor || loadingMoreSessions || !hasMoreSessions) return;
    setLoadingMoreSessions(true);
    try {
      const res = await api.listAssistantSessions(assistantId, {
        cursor: nextSessionCursor,
        limit: 20,
      });
      setSessions((prev) => [...prev, ...res.items]);
      setNextSessionCursor(res.next_cursor ?? null);
      setHasMoreSessions(!res.end && Boolean(res.next_cursor));
    } catch (err) {
      setFailure(`加载更多会话失败：${String(err)}`);
    } finally {
      setLoadingMoreSessions(false);
    }
  };

  useEffect(() => {
    void loadDetail();
    void loadSessions();
  }, [loadDetail, loadSessions]);

  const handleSaveInstructions = async (forceOverwrite = false) => {
    if (!data || savingInstructions) return;
    setSavingInstructions(true);
    setFailure("");
    const revisionToSubmit = forceOverwrite
      ? data.profile.revision
      : (draftBaseRevision.current ?? data.profile.revision);

    try {
      const updated = await api.updateAssistantProfile(
        data.profile.id,
        revisionToSubmit,
        { instructions: instructionsInput.trim() },
      );
      setData(updated);
      assistantDetailCache.set(assistantId, updated);
      clearAssistantCardsCache();
      setInstructionsModified(false);
      draftBaseRevision.current = updated.profile.revision;
      setDraftConflictNotice("");
    } catch (err) {
      setFailure(`保存指令失败：${String(err)}`);
    } finally {
      setSavingInstructions(false);
    }
  };

  const handleDiscardDraft = () => {
    if (!data) return;
    setInstructionsInput(data.profile.instructions);
    setInstructionsModified(false);
    draftBaseRevision.current = data.profile.revision;
    setDraftConflictNotice("");
  };

  const handlePrepareFiles = async () => {
    if (!data || syncingFiles) return;
    setSyncingFiles(true);
    setFailure("");
    try {
      const updated = await api.prepareAssistantFiles(
        data.profile.id,
        data.profile.revision,
      );
      setData(updated);
      assistantDetailCache.set(assistantId, updated);
    } catch (err) {
      setFailure(`同步入口文件失败：${String(err)}`);
    } finally {
      setSyncingFiles(false);
    }
  };

  const handleStartSession = async () => {
    if (!data || startingSession) return;
    setStartingSession(true);
    setFailure("");
    try {
      const launch = await api.startAssistantSession(data.profile.id);
      if (launch.terminal_id) {
        navigate({
          view: "terminal",
          terminalId: launch.terminal_id,
          initialTitle: data.profile.name,
          initialAgent: data.profile.agent,
        });
      }
    } catch (err) {
      setFailure(`启动会话失败：${String(err)}`);
    } finally {
      setStartingSession(false);
    }
  };

  const handleOpenRespModal = async () => {
    if (!data) return;
    setSelectedProjectIds(data.projects.map((p) => p.id));
    setSelectedTaskIds(data.tasks.map((t) => t.id));
    setRespModalOpen(true);
    try {
      const [pList, tList] = await Promise.all([
        api.listProjects(),
        api.listWorkstreams(),
      ]);
      setAvailableProjects(pList);
      setAvailableTasks(tList);
    } catch (err) {
      setFailure(`读取项目/任务失败：${String(err)}`);
    }
  };

  const handleSaveResp = async () => {
    if (!data || savingResp) return;
    setSavingResp(true);
    setFailure("");
    try {
      const updated = await api.setAssistantResponsibilities(
        data.profile.id,
        data.profile.revision,
        selectedProjectIds,
        selectedTaskIds,
      );
      setData(updated);
      assistantDetailCache.set(assistantId, updated);
      clearAssistantCardsCache();
      setRespModalOpen(false);
    } catch (err) {
      setFailure(`保存责任分配失败：${String(err)}`);
    } finally {
      setSavingResp(false);
    }
  };

  const handleToggleArchive = async () => {
    if (!data || archiveBusy) return;
    setArchiveBusy(true);
    setFailure("");
    const nextArchived = data.profile.visibility !== "archived";
    try {
      const updated = await api.setAssistantArchived(
        data.profile.id,
        data.profile.revision,
        nextArchived,
      );
      setData(updated);
      assistantDetailCache.set(assistantId, updated);
      clearAssistantCardsCache();
      setArchiveModalOpen(false);
      notifyDataChanged([]);
    } catch (err) {
      setFailure(`修改归档状态失败：${String(err)}`);
    } finally {
      setArchiveBusy(false);
    }
  };

  const getAvatarIcon = (token: string): IconName => {
    if (token === "agent" || token === "chat" || token === "tasks" || token === "desktop") {
      return token;
    }
    return "spark";
  };

  const isArchived = data?.profile.visibility === "archived";
  const title = data?.profile.name || initialTitle || "智能体详情";

  return (
    <DetailPanelProvider kind="assistants">
      <div className="main detail-page assistant-detail-page">
        <PageHeader
          title={title}
          icon={<Icon name={getAvatarIcon(data?.profile.avatar ?? "spark")} />}
          onBack={() => goBack({ view: "assistants" })}
          actions={
            data && (
              <>
                <button
                  className="btn ghost"
                  onClick={() => setArchiveModalOpen(true)}
                  title={isArchived ? "恢复智能体" : "归档智能体"}
                >
                  <Icon name={isArchived ? "unarchive" : "archive"} />
                  {isArchived ? "恢复" : "归档"}
                </button>
                {!isArchived && (
                  <button
                    className="btn solid"
                    disabled={startingSession}
                    onClick={handleStartSession}
                    title="在智能体专属空间拉起交流会话"
                  >
                    <Icon name="chat" />
                    {startingSession ? "启动中…" : "开始交流"}
                  </button>
                )}
              </>
            )
          }
        />

        {failure && (
          <div style={{ padding: "0 var(--space-lg)" }}>
            <Notice tone="error">
              {failure}
            </Notice>
          </div>
        )}

        {showSkeleton && (
          <div className="detail-loading-skeleton" style={{ padding: "var(--space-lg)" }}>
            <div className="board-card skeleton" style={{ height: "200px" }} />
          </div>
        )}

        {!showSkeleton && data && (
          <DetailLayout
            label="智能体属性栏"
            panel={
              <>
                <DetailSection
                  sectionId="assistant-props"
                  label="档案属性"
                  title="档案属性"
                  actions={
                    !isArchived && (
                      <button
                        className="btn ghost small"
                        onClick={() => setEditModalOpen(true)}
                        title="编辑档案基本信息与运行配置"
                      >
                        <Icon name="edit" /> 编辑资料
                      </button>
                    )
                  }
                >
                  <DetailFields>
                    <Field label="名称">
                      <span className="bold">{data.profile.name}</span>
                    </Field>
                    <Field label="职责简介">
                      <span className="small">{data.profile.description || "暂无简介"}</span>
                    </Field>
                    <Field label="默认运行工具">
                      <span className="badge-agent">
                        <Icon name="agent" /> {data.profile.agent}
                      </span>
                    </Field>
                    <Field label="模型与推理">
                      <span className="small">
                        {data.profile.runtime_override?.model
                          ? `${data.profile.runtime_override.model}${data.profile.runtime_override.effort ? ` (${data.profile.runtime_override.effort})` : ""}`
                          : "Agent 默认"}
                      </span>
                    </Field>
                    <Field label="主目录位置" copyValue={data.workspace_path.canonical_path}>
                      <CompactPath path={data.workspace_path.canonical_path} />
                    </Field>
                    <Field label="档案 ID" copyValue={data.profile.id}>
                      <code>{data.profile.id}</code>
                    </Field>
                    <Field label="创建时间">
                      <span className="small">{timeAgo(data.profile.created_at)}</span>
                    </Field>
                  </DetailFields>
                </DetailSection>

                {/* 专属技能配置 */}
                <DetailSection sectionId="assistant-skills" label="专属技能配置" title="专属技能配置">
                  <div className="muted x-small" style={{ marginBottom: "var(--space-sm)" }}>
                    在此为该智能体独立项目启用或停用技能：
                  </div>
                  <SkillScopeConfig
                    projectId={data.project.id}
                    workspacePaths={[data.workspace_path.canonical_path]}
                    navigate={navigate}
                  />
                </DetailSection>
              </>
            }
          >
            {isArchived && (
              <div className="notice warning" style={{ marginBottom: "var(--space-md)" }}>
                当前智能体已归档。归档期间不可新建管理会话与添加责任，已有历史与资料完整保留。
              </div>
            )}

            {/* 1. 角色设定与准则 */}
            <DetailSection
              sectionId="assistant-instructions"
              label="角色设定与指令"
              title="角色设定与指令 (Instructions)"
              status={
                <span className={`tag small ${data.instructions_state === "current" ? "success" : "warning"}`}>
                  {data.instructions_state === "current" ? "已同步" : "需同步"}
                </span>
              }
              actions={
                <>
                  {data.instructions_state !== "current" && (
                    <button
                      className="btn ghost small"
                      disabled={syncingFiles || isArchived}
                      onClick={handlePrepareFiles}
                      title="重新生成并同步磁盘入口文件 (AGENTS.md 等)"
                    >
                      <Icon name="refresh" /> {syncingFiles ? "同步中…" : "同步入口文件"}
                    </button>
                  )}
                  <button
                    className="btn small solid"
                    disabled={!instructionsModified || savingInstructions || isArchived}
                    onClick={() => void handleSaveInstructions(false)}
                  >
                    {savingInstructions ? "保存中…" : "保存修改"}
                  </button>
                </>
              }
            >
              <div className="instructions-editor">
                {draftConflictNotice && (
                  <div style={{ marginBottom: "var(--space-sm)" }}>
                    <Notice tone="warning">
                      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "var(--space-sm)", flexWrap: "wrap" }}>
                        <span>{draftConflictNotice}</span>
                        <div style={{ display: "flex", gap: "var(--space-xs)" }}>
                          <button
                            type="button"
                            className="btn small ghost"
                            onClick={handleDiscardDraft}
                          >
                            放弃草稿
                          </button>
                          <button
                            type="button"
                            className="btn small solid"
                            disabled={savingInstructions}
                            onClick={() => void handleSaveInstructions(true)}
                          >
                            覆盖远端修改
                          </button>
                        </div>
                      </div>
                    </Notice>
                  </div>
                )}
                <div className={`instructions-status-bar ${data.instructions_state}`}>
                  <span>
                    入口状态：
                    {data.instructions_state === "current"
                      ? "磁盘入口文件已同步"
                      : data.instructions_state === "missing"
                      ? "尚未生成磁盘指令入口文件"
                      : data.instructions_state === "conflict"
                      ? "磁盘文件与当前设定存在冲突"
                      : "入口文件已过时"}
                  </span>
                </div>

                <textarea
                  className="input textarea"
                  rows={6}
                  value={instructionsInput}
                  disabled={isArchived}
                  placeholder="编写该智能体的角色人设、行为准则、交流风格与任务调度规范…"
                  onChange={(e) => {
                    if (!instructionsModified) {
                      draftBaseRevision.current = data?.profile.revision ?? null;
                    }
                    setInstructionsInput(e.target.value);
                    setInstructionsModified(true);
                  }}
                />
                <span className="muted x-small">
                  系统将根据当前绑定的运行工具，自动在智能体主目录生成对应的适配文件（如 AGENTS.md / CLAUDE.md）。
                </span>
              </div>
            </DetailSection>

            {/* 2. 负责工程项目与跟踪任务 */}
            <DetailSection
              sectionId="assistant-responsibilities"
              label="负责项目与跟踪任务"
              title="负责项目与跟踪任务"
              status={
                <span className="muted small">
                  {data.tasks.length + data.projects.length} 项
                </span>
              }
              actions={
                !isArchived && (
                  <button
                    className="btn ghost small"
                    onClick={handleOpenRespModal}
                    title="设置该智能体负责的工程项目与跟踪的任务"
                  >
                    <Icon name="edit" /> 调整分配
                  </button>
                )
              }
            >
              {data.projects.length === 0 && data.tasks.length === 0 ? (
                <div className="muted small" style={{ padding: "var(--space-sm) 0" }}>
                  暂未分配负责的项目或任务。点击右上角「调整分配」为智能体指派管辖范围。
                </div>
              ) : (
                <DetailList label="负责项目与任务列表">
                  {data.projects.map((proj) => (
                    <DetailListRow
                      key={`proj-${proj.id}`}
                      icon={<Icon name="project" />}
                      title={proj.name}
                      description="负责工程项目"
                      actions={
                        <button
                          className="btn ghost small"
                          onClick={() => navigate({ view: "project", projectId: proj.id })}
                        >
                          查看项目
                        </button>
                      }
                    />
                  ))}
                  {data.tasks.map((task) => (
                    <DetailListRow
                      key={`task-${task.id}`}
                      icon={<Icon name="tasks" />}
                      title={task.title}
                      description={`跟踪任务 · 活跃于 ${timeAgo(task.updated_at)}`}
                      actions={
                        <button
                          className="btn ghost small"
                          onClick={() =>
                            navigate({ view: "workstream", workstreamId: task.id })
                          }
                        >
                          查看任务
                        </button>
                      }
                    />
                  ))}
                </DetailList>
              )}
            </DetailSection>

            {/* 3. 历史管理会话 */}
            <DetailSection
              sectionId="assistant-sessions"
              label="会话记录"
              title="会话记录"
              status={<span className="muted small">{sessions.length} 次</span>}
              actions={
                <button
                  className="btn ghost small icon-button"
                  onClick={() => void loadSessions()}
                  disabled={loadingSessions}
                  title="刷新会话列表"
                >
                  <Icon name="refresh" />
                </button>
              }
            >
              {sessions.length === 0 ? (
                <div className="muted small" style={{ padding: "var(--space-sm) 0" }}>
                  该智能体尚未产生任何会话。点击上方「开始交流」拉起首次会话。
                </div>
              ) : (
                <DetailList label="会话列表">
                  {sessions.map((sess) => (
                    <DetailListRow
                      key={sess.id}
                      icon={<Icon name="chat" />}
                      title={sess.title || "未命名会话"}
                      description={`${sess.agent} · ${timeAgo(sess.last_activity_at)}`}
                      actions={
                        <button
                          className="btn ghost small"
                          onClick={() =>
                            navigate({ view: "session", sessionId: sess.id })
                          }
                        >
                          进入会话
                        </button>
                      }
                    />
                  ))}
                </DetailList>
              )}

              {hasMoreSessions && (
                <div style={{ display: "flex", justifyContent: "center", paddingTop: "var(--space-md)" }}>
                  <button
                    className="btn ghost small"
                    disabled={loadingMoreSessions}
                    onClick={handleLoadMoreSessions}
                  >
                    {loadingMoreSessions ? "加载中…" : "加载更多会话"}
                  </button>
                </div>
              )}
            </DetailSection>
          </DetailLayout>
        )}

        {/* 编辑档案基本信息与运行配置 Modal */}
        {data && (
          <EditProfileModal
            open={editModalOpen}
            profile={data.profile}
            onClose={() => setEditModalOpen(false)}
            onSaved={(updated) => {
              setData(updated);
              assistantDetailCache.set(assistantId, updated);
              clearAssistantCardsCache();
              notifyDataChanged([]);
            }}
          />
        )}

        {/* 责任范围分配 Modal */}
        {respModalOpen && (
          <Modal
            title="调整负责项目与任务"
            onClose={() => setRespModalOpen(false)}
          >
            <div className="responsibilities-modal-content">
              <div className="responsibilities-modal-section">
                <span className="label bold">负责工程项目</span>
                <div className="responsibilities-checklist">
                  {availableProjects.length === 0 ? (
                    <span className="muted small">暂无可分配项目</span>
                  ) : (
                    availableProjects.map((p) => {
                      const checked = selectedProjectIds.includes(p.id);
                      return (
                        <label key={p.id} className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={(e) => {
                              if (e.target.checked) {
                                setSelectedProjectIds([...selectedProjectIds, p.id]);
                              } else {
                                setSelectedProjectIds(
                                  selectedProjectIds.filter((id) => id !== p.id),
                                );
                              }
                            }}
                          />
                          <span>{p.name}</span>
                        </label>
                      );
                    })
                  )}
                </div>
              </div>

              <div className="responsibilities-modal-section">
                <span className="label bold">跟踪工程任务</span>
                <div className="responsibilities-checklist">
                  {availableTasks.length === 0 ? (
                    <span className="muted small">暂无可分配任务</span>
                  ) : (
                    availableTasks.map((t) => {
                      const checked = selectedTaskIds.includes(t.id);
                      return (
                        <label key={t.id} className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={(e) => {
                              if (e.target.checked) {
                                setSelectedTaskIds([...selectedTaskIds, t.id]);
                              } else {
                                setSelectedTaskIds(
                                  selectedTaskIds.filter((id) => id !== t.id),
                                );
                              }
                            }}
                          />
                          <span>{t.title}</span>
                        </label>
                      );
                    })
                  )}
                </div>
              </div>
            </div>

            <DialogActions>
              <button
                className="btn ghost"
                disabled={savingResp}
                onClick={() => setRespModalOpen(false)}
              >
                取消
              </button>
              <button
                className="btn solid"
                disabled={savingResp}
                onClick={handleSaveResp}
              >
                {savingResp ? "保存中…" : "确认分配"}
              </button>
            </DialogActions>
          </Modal>
        )}

        {/* 归档 / 恢复确认 Modal */}
        {archiveModalOpen && data && (
          <Modal
            title={isArchived ? "恢复智能体" : "归档智能体"}
            onClose={() => setArchiveModalOpen(false)}
          >
            <p className="body" style={{ margin: "var(--space-md) 0" }}>
              {isArchived
                ? `确定要恢复智能体「${data.profile.name}」吗？恢复后可继续拉起新会话并管理任务。`
                : `确定要归档智能体「${data.profile.name}」吗？归档后将暂停新会话启动，其专属目录、文件资料及所有历史会话均完整保留。`}
            </p>
            <DialogActions>
              <button
                className="btn ghost"
                disabled={archiveBusy}
                onClick={() => setArchiveModalOpen(false)}
              >
                取消
              </button>
              <button
                className={`btn solid${isArchived ? "" : " danger"}`}
                disabled={archiveBusy}
                onClick={handleToggleArchive}
              >
                {archiveBusy
                  ? isArchived
                    ? "恢复中…"
                    : "归档中…"
                  : isArchived
                  ? "确认恢复"
                  : "确认归档"}
              </button>
            </DialogActions>
          </Modal>
        )}
      </div>
    </DetailPanelProvider>
  );
}
