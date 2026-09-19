import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { timeAgo } from "../../components/common";
import { useRefreshSignal, Modal } from "../../components/common";
import { IntelligenceOnly } from "../../app/experience";
import NewSessionModal from "../sessions/NewSessionModal";
import ResumeSessionModal from "../sessions/ResumeSessionModal";
import { cwdDisplayLabel } from "../sessions/SessionTable";
import {
  AGENT_LABELS,
  type Agent,
  type Workstream,
  type WorkstreamContext as WorkstreamContextData,
  type WorkstreamLifecycle,
  type WorkstreamPathRow,
  type WorkstreamReviewSummary,
  type WorkstreamReviewWindow,
} from "../../types";
import type { Route, WorkstreamEntry } from "../../app/routes";
import WorkstreamContext from "./WorkstreamContext";
import WorkstreamSessions from "./WorkstreamSessions";
import WorkstreamPathList from "./WorkspacePaths";
import NeedsAttentionSection from "./NeedsAttentionSection";
import ConflictReviewModal from "./ConflictReviewModal";
import RecentChangesTimeline from "./RecentChangesTimeline";
import SinceLastReview from "./SinceLastReview";

/**
 * Workstream Detail = 持续相关 Sessions 的组织容器（方案 §14）。
 *
 * Base Experience 下这一页只有五块内容：概览（描述）、Sessions、工作路径、
 * Project、状态。Context 智能段落（Current Context / Since Last Review /
 * Needs Attention / Recent Changes / Conflict Review）全部保留代码但不挂载
 * ——见 §11.9，off 就是 `<IntelligenceOnly>` 里不渲染。
 *
 * 启动路径唯一：本页不再自己调 launcher，而是挂载 New Session / Resume 的
 * 同一个 Modal（§8.1.1 契约），由它们走 prepare → 状态指纹 → launch_prepared
 * （Preview-Launch Identity / Launch Preparation Integrity）。
 *
 * v0.2 的三条边界（§1.5 / §1.13 / §42.3-M19）：
 *   • 工作目录 = **有序 WorkstreamPath 列表**，`default_cwd` 只作为冻结的迁移输入
 *     留在类型里，本页不再读写它；
 *   • Project 是**只读投影**（主路径 → WorkspacePath → Project），所以链接取的是
 *     列表第 1 条路径所属 Project，而不是 `workstreams.project_id` 那个兼容列；
 *   • 归档 / 恢复 / 永久删除是三个**单向**命令，不是一枚翻转开关。
 */
export default function WorkstreamDetailView({
  workstreamId,
  entry,
  navigate,
}: {
  workstreamId: string;
  entry?: WorkstreamEntry;
  navigate: (r: Route) => void;
}) {
  const [ctx, setCtx] = useState<WorkstreamContextData | null>(null);
  const [paths, setPaths] = useState<WorkstreamPathRow[] | null>(null);
  const [pathsError, setPathsError] = useState("");
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const [titleOpen, setTitleOpen] = useState(false);
  const [titleInput, setTitleInput] = useState("");
  const [editingDescription, setEditingDescription] = useState(false);
  const [descriptionInput, setDescriptionInput] = useState("");
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [resumeSessionId, setResumeSessionId] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const [purgeConfirmText, setPurgeConfirmText] = useState("");
  const busyRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // ••• 菜单：点击外部与 Escape 都要收起。之前只有再点一次 ••• 才关得掉，
  // 点别处它一直悬着（§25 交互一致性）。mousedown 阶段监听，先于 click，
  // 所以菜单项自己的 click 仍然正常触发。
  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    let cancelled = false;
    // 换 Workstream 时先把手里的投影清空：本页所有写操作都以 workstreamId 为准，
    // 留着上一条的路径列表会让用户在错的列表上按下按钮（例如把旧 path id 发给
    // 新 Workstream 的 reorder）。宁可闪一下「加载中…」，也不拿旧数据当现状。
    setCtx(null);
    setPaths(null);
    setPathsError("");
    setMenuOpen(false);
    setConfirmTrash(false);
    setConfirmPurge(false);
    setActionError("");
    setError("");
    api.getWorkstreamContext(workstreamId).then((c) => {
      if (!cancelled) setCtx(c);
    }).catch(console.error);
    api.listWorkstreamPaths(workstreamId).then((rows) => {
      if (!cancelled) { setPaths(rows); setPathsError(""); }
    }).catch((e) => {
      if (!cancelled) { setPaths(null); setPathsError(`读取工作路径失败：${String(e)}`); }
    });
    return () => {
      cancelled = true;
    };
  }, [workstreamId]);

  useEffect(() => {
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
  }, []);

  // Background refresh only re-reads the projection; the intelligence panels
  // own their own (frozen-window) refresh so this page never touches ReviewState.
  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
    api.listWorkstreamPaths(workstreamId).then((rows) => {
      setPaths(rows); setPathsError("");
    }).catch((e) => {
      setPaths(null); setPathsError(`读取工作路径失败：${String(e)}`);
    });
  }, [workstreamId]);

  useRefreshSignal(refresh);

  if (!ctx) return <div className="main narrow">加载中…</div>;
  const { workstream, related_sessions } = ctx;

  const latest = [...related_sessions]
    .sort((a, b) =>
      (b.last_activity_at ?? b.started_at ?? "").localeCompare(
        a.last_activity_at ?? a.started_at ?? "",
      ),
    )[0];

  /** 一条命令返回新的 Workstream 时立刻就地替换：`update_workstream` 是整对象写，
   *  手里留着旧的 lifecycle / visibility 会让下一次保存被后端拒绝。 */
  const adopt = (next: Workstream) => setCtx((c) => (c ? { ...c, workstream: next } : c));

  /**
   * `update_workstream` is a whole-object write, so every edit re-sends the
   * current record with one field replaced — that is why only 标题 / 描述 走这条路：
   * lifecycle、visibility、project_id、default_cwd 都有自己的命令或被后端冻结
   * （§42.2-E6：`apply_whole_object_edit` 会直接拒绝携带改动过 lifecycle 的对象）。
   *
   * title / description / updated_at are part of the launch state fingerprint
   * (launcher/mod.rs:475-478): an edit here intentionally invalidates any
   * not-yet-consumed PreparedLaunch, which surfaces as 「状态已变化」 in the
   * launch modal rather than being silently absorbed.
   */
  const save = async (patch: Partial<Pick<Workstream, "title" | "description">>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      await api.updateWorkstream({ ...workstream, ...patch });
      refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /**
   * §1.13：lifecycle 只是分类，没有任何行为差异，随时可切；它不碰路径、
   * 绑定、visibility 或 Context。
   */
  const setLifecycle = async (next: WorkstreamLifecycle) => {
    if (busyRef.current || workstream.lifecycle === next) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.setWorkstreamLifecycle(workstream.id, next));
      refresh();
    } catch (e) {
      console.error(e);
      setActionError(`修改状态失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /**
   * 归档与恢复是**两个单向命令**（§43.4-1）：`archive_workstream` 只进回收站，
   * `restore_workstream` 只出来。旧代码把 archive 当翻转用，于是「再点一次取消
   * 归档」和「重复点击」会互相抵消 —— 那正是 v0.2 要消掉的双权威。
   * 之前这里 `await` 完不判成败就跳走：失败时菜单收起、页面不动、没有任何
   * 回执。现在失败留在原地说明原因，成功才跳走。
   */
  const moveToTrash = async () => {
    setMenuOpen(false);
    setConfirmTrash(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      adopt(await api.archiveWorkstream(workstream.id));
      navigate({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`移入回收站失败：${String(e)}`);
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const restore = async () => {
    setMenuOpen(false);
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

  /** 唯一不可逆的动作，且后端只接受从回收站出发（§1.13）。 */
  const purge = async () => {
    setConfirmPurge(false);
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    try {
      await api.deleteWorkstreamPermanently(workstream.id);
      navigate({ view: "workstreams" });
    } catch (e) {
      console.error(e);
      setActionError(`永久删除失败：${String(e)}`);
      setPurgeConfirmText("");
      refresh();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const openTitleEditor = () => {
    setMenuOpen(false);
    setTitleInput(workstream.title);
    setTitleOpen(true);
  };

  const openDescriptionEditor = () => {
    setMenuOpen(false);
    setDescriptionInput(workstream.description ?? "");
    setEditingDescription(true);
  };

  const saveTitle = async () => {
    const trimmed = titleInput.trim();
    if (!trimmed || trimmed === workstream.title) {
      setTitleOpen(false);
      return;
    }
    setTitleOpen(false);
    await save({ title: trimmed });
  };

  const saveDescription = async () => {
    setEditingDescription(false);
    await save({ description: descriptionInput.trim() });
  };

  const archived = workstream.visibility === "archived";
  // §42.3-M19：Project 只从**主工作路径**投影出来。`workstream.project_id` 是
  // v0.2 冻结的兼容列，拿它导航会跳到一条没人手工指派过的旧成员关系上。
  const primary = paths?.[0] ?? null;
  // §1.12：一个 Workstream 可以因为不同路径同时出现在多个 Project 里；
  // 经由 position 0 那条路径到达的才是「主关联」。
  const projectRows = (() => {
    const byId = new Map<string, { id: string; name: string | null; primary: boolean; count: number }>();
    for (const p of paths ?? []) {
      const seen = byId.get(p.project_id);
      byId.set(p.project_id, {
        id: p.project_id,
        name: p.project_name ?? seen?.name ?? null,
        primary: (seen?.primary ?? false) || p.position === 0,
        count: (seen?.count ?? 0) + 1,
      });
    }
    return [...byId.values()].sort((a, b) => (b.primary ? 1 : 0) - (a.primary ? 1 : 0));
  })();

  return (
    <div className="main narrow">
      <PageHeader
        back="Workstreams"
        onBack={() => navigate({ view: "workstreams" })}
        title={<span style={{ overflowWrap: "anywhere" }}>{workstream.title}</span>}
        actions={
          <>
            <div style={{ position: "relative" }} ref={menuRef}>
              <button className="btn ghost" onClick={() => setMenuOpen((v) => !v)} title="更多操作"
                aria-haspopup="true" aria-expanded={menuOpen}>
                •••
              </button>
              {menuOpen && (
                <div className="menu-pop">
                  <button className="menu-item" onClick={openTitleEditor}>
                    重命名…
                  </button>
                  <button className="menu-item" onClick={openDescriptionEditor}>
                    编辑描述…
                  </button>
                  {/* 不写 disabled={busy}：全局样式只给了 `button.btn:disabled`
                      一种弱化态（global.css:56），`.menu-item` 上的 disabled 既
                      不变灰也不换 cursor，等于"看着能点、点了没反应"。真正的
                      重复提交由各动作开头的 busyRef 守卫挡掉。 */}
                  {archived ? (
                    <>
                      <button className="menu-item" onClick={restore}>
                        从回收站恢复
                      </button>
                      <button className="menu-item"
                        title="不可撤销：会删除这条 Workstream 名下的 Context、冲突记录与审阅状态。Session 与它们的事件历史保留。"
                        onClick={() => { setMenuOpen(false); setPurgeConfirmText(""); setConfirmPurge(true); }}>
                        永久删除…
                      </button>
                    </>
                  ) : (
                    <button className="menu-item"
                      title="移入回收站：只是不再出现在列表里，路径、绑定与 Context 都原样保留，随时可以恢复。"
                      onClick={() => { setMenuOpen(false); setConfirmTrash(true); }}>
                      移入回收站…
                    </button>
                  )}
                </div>
              )}
            </div>
            {/* 与 Home / Workstream 卡片同一个判断：这一页不自己宣称「没有 Agent」。
                defaultAgent 是本页异步读回来的，解析期间 disabled + 「未检测到」
                的 tooltip 会说假话；真正判定交给 NewSessionModal（唯一启动路径）。 */}
            <button
              className="btn ws-btn"
              title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建 Session` : "新建 Session"}
              onClick={() => setNewSessionOpen(true)}
            >
              {defaultAgent ? <AgentIcon agent={defaultAgent} /> : null}
              新建 Session
            </button>
            {latest && (
              <button
                className="btn primary ws-btn resume-primary"
                title={`继续最近的 ${AGENT_LABELS[latest.agent]} Session`}
                onClick={() => setResumeSessionId(latest.id)}
              >
                <AgentIcon agent={latest.agent} />
                继续
              </button>
            )}
          </>
        }
      >
        <div className="ws-detail-head-meta">
          {primary?.project_id && (
            <>
              {/* Project 是主工作路径的派生投影（§42.3-M19）：这里既不能改，也没有
                  「换一个 Project」这回事 —— 想换 Project，改的是工作路径列表。 */}
              <button className="link" title={`由主工作路径派生的 Project（只读）\n${primary.canonical_path}\n→ ${primary.project_name ?? primary.project_id}`}
                style={{ maxWidth: 180, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                onClick={() => navigate({ view: "project", projectId: primary.project_id })}>
                {primary.project_name ?? "Project"}
              </button>
              <span className="dot-sep" />
            </>
          )}
          {/* lifecycle 与 visibility 是两个正交维度（方案 §1.13），所以这里给两枚
              徽标，而不是让「回收站」盖掉「进行中 / 已完成」。 */}
          <span className={`badge ${workstream.lifecycle === "active" ? "success" : ""}`}>
            {LIFECYCLE_LABELS[workstream.lifecycle] ?? workstream.lifecycle}
          </span>
          {archived && (
            <span className="badge warn" title="回收站：路径、Session 绑定与 Context 都原样保留，随时可以恢复">
              回收站
            </span>
          )}
          {primary && (
            <>
              <span className="dot-sep" />
              {/* 这一行是不换行的 flex：超长路径（Windows 深路径、中文目录）会把
                  后面的状态与「最近更新」挤出屏幕。中段省略，完整路径留在 title。 */}
              <span
                className="mono small"
                title={paths && paths.length > 1
                  ? `主工作路径（新建 Session 默认在这里启动）\n${primary.canonical_path}\n另有 ${paths.length - 1} 条工作路径`
                  : `主工作路径（新建 Session 默认在这里启动）\n${primary.canonical_path}`}
              >
                {cwdDisplayLabel(primary.canonical_path, 30)}
              </span>
              {!primary.exists && <span className="badge warn" title={primary.canonical_path}>主路径目录不存在</span>}
            </>
          )}
          {paths !== null && paths.length === 0 && (
            <>
              <span className="dot-sep" />
              <span className="small muted" title="没有工作路径是合法状态：新建 Session 会在 NoEnding 默认工作目录启动">
                无工作路径
              </span>
            </>
          )}
          <span className="dot-sep" />
          <span>最近更新 {timeAgo(workstream.updated_at)}</span>
        </div>
        {(error || actionError) && (
          <div className="small" style={{ color: "var(--warning)", marginTop: 6, overflowWrap: "anywhere" }}>
            {actionError || `保存失败：${error}`}
          </div>
        )}
      </PageHeader>

      {/* ---------- Base Experience：Workstream 自身（§14） ---------- */}
      <div style={{ marginTop: 26 }}>
        <section className="rail-section">
          <div className="rail-head">
            <div className="section-label" style={{ margin: 0 }}>Workstream 概览</div>
            {!editingDescription && (
              <button className="link" onClick={openDescriptionEditor}>编辑描述</button>
            )}
          </div>
          {editingDescription ? (
            <div className="ctx-edit">
              <textarea
                value={descriptionInput}
                autoFocus
                placeholder="这件 Workstream 想持续做什么（可选）"
                onChange={(e) => setDescriptionInput(e.target.value)}
              />
              <div className="row" style={{ justifyContent: "flex-end" }}>
                <button className="btn" onClick={() => setEditingDescription(false)}>取消</button>
                <button className="btn primary" disabled={busy} onClick={saveDescription}>保存</button>
              </div>
            </div>
          ) : workstream.description ? (
            <p style={{ margin: 0, maxWidth: "72ch", overflowWrap: "anywhere" }}>
              {workstream.description}
            </p>
          ) : (
            <div className="l1-none">还没有描述。</div>
          )}
        </section>

        <WorkstreamSessions
          sessions={related_sessions}
          navigate={navigate}
          onNewSession={() => setNewSessionOpen(true)}
        />

        <WorkstreamPathList
          workstreamId={workstreamId}
          paths={paths}
          error={pathsError}
          onChanged={refresh}
        />

        <section className="rail-section">
          <div className="section-label">Project</div>
          {paths === null && (
            <div className="muted small">{pathsError || "读取工作路径后才能确定…"}</div>
          )}
          {paths !== null && projectRows.length === 0 && (
            <div className="l1-none">
              这条 Workstream 还没有工作路径，所以也不归属任何 Project —— 这是合法状态。
            </div>
          )}
          {paths !== null && projectRows.map((p) => (
            <div className="list-row" key={p.id} style={{ cursor: "default" }}>
              <div className="grow">
                <div className="title" title={p.name ?? p.id}>{p.name ?? "未命名 Project"}</div>
                <div className="meta">
                  {p.primary ? "主关联 — 经由主工作路径到达" : "关联 — 经由其他工作路径到达"}
                  {p.count > 1 ? ` · ${p.count} 条路径` : ""}
                </div>
              </div>
              <div className="side">
                <button className="btn small" onClick={() => navigate({ view: "project", projectId: p.id })}>
                  打开
                </button>
              </div>
            </div>
          ))}
          <div className="small muted" style={{ marginTop: 4 }}>
            Project 是<b>只读投影</b>（工作路径 → WorkspacePath → Project，方案 §1.12）：
            它不能在这里改，也没有「换一个 Project」这个操作 —— 想改变归属，改的是上面的工作路径列表。
          </div>
        </section>

        <section className="rail-section">
          <div className="section-label">状态</div>
          <div className="row" style={{ gap: 8 }}>
            <button className={`btn small ${workstream.lifecycle === "active" ? "primary" : ""}`}
              disabled={busy || workstream.lifecycle === "active"}
              onClick={() => setLifecycle("active")}>
              进行中
            </button>
            <button className={`btn small ${workstream.lifecycle === "completed" ? "primary" : ""}`}
              disabled={busy || workstream.lifecycle === "completed"}
              onClick={() => setLifecycle("completed")}>
              已完成
            </button>
            {archived && <span className="badge warn">在回收站中</span>}
          </div>
          <div className="small muted" style={{ marginTop: 6, maxWidth: "72ch" }}>
            进行中 / 已完成 只是分类标签：两者没有任何行为差异，随时可以来回切换，
            也不会触碰工作路径、Session 绑定或 Context。
          </div>
          <div className="small muted" style={{ marginTop: 6, maxWidth: "72ch" }}>
            {archived ? (
              <>
                这条 Workstream 在回收站里，路径、绑定与 Context 都原样保留。
                <button className="link" style={{ marginLeft: 4 }} onClick={restore} disabled={busy}>恢复</button>
                <button className="link" style={{ marginLeft: 10 }} onClick={() => { setPurgeConfirmText(""); setConfirmPurge(true); }}
                  disabled={busy}>永久删除…</button>
              </>
            ) : (
              <>回收站：从 ••• 菜单「移入回收站」；它只是不再出现在列表里，随时可以恢复。</>
            )}
          </div>
          <div className="small muted" style={{ marginTop: 6 }}>
            创建于 {formatDate(workstream.created_at)} · 最近更新 {timeAgo(workstream.updated_at)}
          </div>
        </section>
      </div>

      {/* ---------- 智能段落：off 时整块不挂载（§11.9、§14） ---------- */}
      <IntelligenceOnly>
        <div className="ws-detail-grid">
          <IntelligenceSections
            ctx={ctx}
            entry={entry}
            workstreamId={workstreamId}
            navigate={navigate}
            onChanged={refresh}
          />
        </div>
      </IntelligenceOnly>

      {newSessionOpen && (
        <NewSessionModal
          workstreamId={workstream.id}
          onClose={() => {
            setNewSessionOpen(false);
            refresh();
          }}
        />
      )}

      {resumeSessionId && (
        <ResumeSessionModal
          sessionId={resumeSessionId}
          onClose={() => {
            setResumeSessionId(null);
            refresh();
          }}
        />
      )}

      {titleOpen && (
        <Modal title="重命名 Workstream" onClose={() => setTitleOpen(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            标题是用户可见的组织信息。修改会让已经预览过、但还没启动的那次
            Session 变成「状态已变化」，需要你重新确认——这是刻意保留的保护。
          </p>
          <label className="field"><span>标题</span>
            <input type="text" value={titleInput} autoFocus
              onChange={(e) => setTitleInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && saveTitle()} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setTitleOpen(false)}>取消</button>
            <button className="btn primary" disabled={!titleInput.trim()} onClick={saveTitle}>保存</button>
          </div>
        </Modal>
      )}

      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => setConfirmTrash(false)}>
          <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
            <b>{workstream.title}</b> 会离开正常列表，出现在 Workstreams 页的「回收站」筛选里。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            不会删除任何东西：工作路径列表、Session 绑定、Context、审阅状态、lifecycle
            都按原样保留，恢复后回到你离开时的样子（方案 §1.13）。
          </p>
          <p className="small muted" style={{ marginBottom: 12 }}>
            回收站只是收起来，不是删除。要真正删除，需要在回收站里选「永久删除」，
            那一步不可撤销、并且会连带结束这条 Workstream 名下的 Context。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmTrash(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={moveToTrash} disabled={busy}>
              {busy ? "处理中…" : "移入回收站"}
            </button>
          </div>
        </Modal>
      )}

      {confirmPurge && (
        <Modal title="永久删除这条 Workstream？" onClose={() => setConfirmPurge(false)}>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            不可撤销
          </div>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将<b>删除</b>：<span className="mono">{workstream.title}</span> 本身、它的有序工作路径列表、
            它名下的全部 Context 条目与 Revision、冲突记录与解决历史、以及审阅状态（ReviewState）。
          </p>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将<b>保留</b>：它引用过的 Sessions 与这些 Session 的完整事件历史、原始 Agent
            会话文件、启动记录、以及工作目录本身（WorkspacePath）与由它派生的 Project。
            换句话说：Project 与 Session 都不会因为删掉一条 Workstream 而受影响。
          </p>
          <p className="small muted" style={{ marginBottom: 12 }}>
            只有已经在回收站里的 Workstream 才能被永久删除 —— 这是刻意留的缓冲。
          </p>
          <label className="field"><span>输入这条 Workstream 的标题以确认</span>
            <input type="text" value={purgeConfirmText} autoFocus
              placeholder={workstream.title}
              onChange={(e) => setPurgeConfirmText(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && purgeConfirmText === workstream.title && purge()} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmPurge(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={purge}
              disabled={busy || purgeConfirmText !== workstream.title}>
              {busy ? "删除中…" : "确认永久删除"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/**
 * Context 智能段落。单独成一个组件，是为了让这些开关只在「真的挂载」时才发生：
 * getWorkstreamReviewWindow / Summary / markWorkstreamReviewed / conflict 命令在
 * Base Experience 下不出请求（§34），而不是发了请求再藏起来。
 */
function IntelligenceSections({
  ctx,
  entry,
  workstreamId,
  navigate,
  onChanged,
}: {
  ctx: WorkstreamContextData;
  entry?: WorkstreamEntry;
  workstreamId: string;
  navigate: (r: Route) => void;
  onChanged: () => void;
}) {
  const [reviewWindow, setReviewWindow] = useState<WorkstreamReviewWindow | null>(null);
  const [reviewSummary, setReviewSummary] = useState<WorkstreamReviewSummary | null>(null);
  const [reviewDirty, setReviewDirty] = useState(false);
  const [markingReviewed, setMarkingReviewed] = useState(false);
  const [focusedItemId, setFocusedItemId] = useState<string | null>(null);
  const [reviewingConflict, setReviewingConflict] = useState(false);
  const [reviewingConflictId, setReviewingConflictId] = useState<string | undefined>(undefined);

  const reviewWindowRef = useRef<WorkstreamReviewWindow | null>(null);
  reviewWindowRef.current = reviewWindow;
  const consumedEntryRef = useRef(false);

  useEffect(() => {
    consumedEntryRef.current = false;
  }, [workstreamId]);

  // Auto-open conflict review modal if entry === "conflicts" and open conflicts exist (one-shot navigation intent)
  useEffect(() => {
    if (!consumedEntryRef.current && entry === "conflicts" && ctx && (ctx.conflicts?.length ?? 0) > 0) {
      consumedEntryRef.current = true;
      setReviewingConflict(true);
    }
  }, [entry, ctx]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.getWorkstreamReviewWindow(workstreamId),
      api.getWorkstreamReviewSummary(workstreamId),
    ]).then(([w, s]) => {
      if (!cancelled) {
        setReviewWindow(w);
        setReviewSummary(s);
        setReviewDirty(s.unseen_change_count !== w.unseen_changes.length);
      }
    }).catch(console.error);
    return () => {
      cancelled = true;
    };
  }, [workstreamId]);

  // Regular contextual or background refresh:
  // Updates summary, but preserves the frozen reviewWindow!
  const refreshSummary = useCallback(() => {
    api.getWorkstreamReviewSummary(workstreamId).then((s) => {
      setReviewSummary(s);
      const cur = reviewWindowRef.current;
      if (cur && s.unseen_change_count > cur.unseen_changes.length) {
        setReviewDirty(true);
      }
    }).catch(console.error);
  }, [workstreamId]);

  useRefreshSignal(refreshSummary);

  // Explicit user refresh of review window
  const handleRefreshReview = useCallback(async () => {
    try {
      const [w, s] = await Promise.all([
        api.getWorkstreamReviewWindow(workstreamId),
        api.getWorkstreamReviewSummary(workstreamId),
      ]);
      setReviewWindow(w);
      setReviewSummary(s);
      setReviewDirty(s.unseen_change_count !== w.unseen_changes.length);
    } catch (err) {
      console.error("Failed to refresh review window:", err);
    }
  }, [workstreamId]);

  // Mark reviewed action:
  // Critical invariant: uses observed reviewWindow.mark_through, NOT a newly fetched frontier!
  const handleMarkReviewed = useCallback(async () => {
    if (!reviewWindow || markingReviewed) return;
    const observed = reviewWindow;
    setMarkingReviewed(true);
    try {
      await api.markWorkstreamReviewed(workstreamId, observed.mark_through);
      const [nextWindow, nextSummary] = await Promise.all([
        api.getWorkstreamReviewWindow(workstreamId),
        api.getWorkstreamReviewSummary(workstreamId),
      ]);
      setReviewWindow(nextWindow);
      setReviewSummary(nextSummary);
      setReviewDirty(nextSummary.unseen_change_count !== nextWindow.unseen_changes.length);
    } catch (err) {
      console.error("Failed to mark workstream reviewed:", err);
    } finally {
      setMarkingReviewed(false);
    }
  }, [workstreamId, reviewWindow, markingReviewed]);

  return (
    <>
      <div>
        {reviewWindow && reviewSummary && (
          <SinceLastReview
            window={reviewWindow}
            summary={reviewSummary}
            dirty={reviewDirty}
            marking={markingReviewed}
            onMarkReviewed={handleMarkReviewed}
            onRefresh={handleRefreshReview}
            onOpenItem={(itemId) => {
              setFocusedItemId(null);
              setTimeout(() => setFocusedItemId(itemId), 20);
            }}
            onOpenConflict={(conflictId) => {
              setReviewingConflictId(conflictId);
              setReviewingConflict(true);
            }}
          />
        )}
        <WorkstreamContext
          ctx={ctx}
          focusedItemId={focusedItemId}
          onChanged={onChanged}
          onNavigateSession={(sessionId) => navigate({ view: "session", sessionId })}
        />
      </div>
      <div>
        <NeedsAttentionSection
          conflicts={ctx.conflicts ?? []}
          onReview={() => {
            setReviewingConflictId(undefined);
            setReviewingConflict(true);
          }}
        />
        <RecentChangesTimeline changes={ctx.recent_changes ?? []} />
      </div>

      {reviewingConflict && (
        <ConflictReviewModal
          cases={ctx.conflict_cases ?? []}
          initialConflictId={reviewingConflictId}
          onClose={() => {
            setReviewingConflict(false);
            setReviewingConflictId(undefined);
          }}
          onChanged={onChanged}
        />
      )}
    </>
  );
}

// 词表（方案 §22）：lifecycle 只有 进行中 / 已完成 两种；archived 不属于这套词，
// 它是 visibility 那一维，UI 上统一叫「回收站」，所以在这里没有第三个标签。
const LIFECYCLE_LABELS: Record<Workstream["lifecycle"], string> = {
  active: "进行中",
  completed: "已完成",
};

/**
 * RFC3339 (UTC) → 本地 YYYY/MM/DD.
 *
 * 不用 `toLocaleDateString()`：webview 语言不一定是中文，格式会随环境漂移；
 * 也不用 `slice(0, 10)`：那是 UTC 日期，本地可能已经跨了一天。
 */
function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getFullYear()}/${pad(t.getMonth() + 1)}/${pad(t.getDate())}`;
}
