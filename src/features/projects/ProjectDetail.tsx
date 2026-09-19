import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { timeAgo, useRefreshSignal, Modal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { GitStateBadge, MissingBadge, PathError, PathText } from "../workstreams/WorkspacePaths";
import { sessionDisplayTitle, UNTITLED_SESSION, cwdDisplayLabel } from "../sessions/SessionTable";
import { IntelligenceOnly, useBaseExperience } from "../../app/experience";
import { cardSummaryLine } from "../workstreams/WorkstreamCard";
import {
  AGENT_LABELS,
  type ProjectDetailData,
  type WorkstreamCardData,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Project Detail（方案 §11 / §22 / §42.3-M22）。
 *
 * 一次 `get_project_detail` 读到全部真相：它拥有的 WorkspacePaths、经由这些路径
 * 到达的 Workstreams（含「主关联 / 关联」，§1.12）、以及 cwd 落在这些路径上的
 * Sessions（走权威链，不走缓存列，§43.3-M29）。
 *
 * 用户在这页唯一能做的编辑是**改名**：Project 的其余事实都是派生的，
 * 手工新建 / 删除 / 加引用资料 / 移动 Workstream / 移动 Session 都已退出产品 API。
 */
export default function ProjectDetail({ projectId, navigate }: {
  projectId: string;
  navigate: (r: Route) => void;
}) {
  const { intelligenceEnabled } = useBaseExperience();
  const [data, setData] = useState<ProjectDetailData | null>(null);
  const [gone, setGone] = useState(false);
  const [failure, setFailure] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [nameInput, setNameInput] = useState("");
  const [renameError, setRenameError] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const renameBusyRef = useRef(false);
  const seqRef = useRef(0);

  const refresh = useCallback(() => {
    const seq = ++seqRef.current;
    api.getProjectDetail(projectId)
      .then((d) => {
        if (seq !== seqRef.current) return;
        setData(d);
        setGone(false);
        setFailure("");
      })
      .catch((e) => {
        if (seq !== seqRef.current) return;
        const text = String(e);
        setFailure(text);
        // 后端对不存在的 Project 返回 `Project <id> 不存在`（commands/project.rs）。
        // 只按这句话判定「已经消失」，其余一律当作读取失败 —— 猜错的代价是
        // 让用户以为自己的 Project 没了。
        setGone(text.includes(`Project ${projectId} 不存在`));
      });
  }, [projectId]);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const detail = data;
  const project = detail?.project ?? null;

  const commitRename = async () => {
    const next = nameInput.trim();
    if (project === null || renameBusyRef.current) return;
    if (next === "") {
      setRenameError("名字不能为空");
      return;
    }
    if (next === project.name) {
      setRenaming(false);
      return;
    }
    renameBusyRef.current = true;
    setRenameBusy(true);
    setRenameError("");
    try {
      const renamed = await api.renameProject(project.id, next);
      setData((cur) => (cur ? { ...cur, project: renamed } : cur));
      setRenaming(false);
      refresh();
    } catch (e) {
      // 失败留在弹窗里说原因：静默关闭会让用户以为已经改好了。
      console.error(e);
      setRenameError(`重命名失败：${String(e)}`);
    } finally {
      renameBusyRef.current = false;
      setRenameBusy(false);
    }
  };

  if (detail === null || project === null) {
    return (
      <div className="main narrow">
        <PageHeader
          back="Projects"
          onBack={() => navigate({ view: "projects" })}
          title={gone ? "这个 Project 已经不存在" : (failure === "" ? "加载中…" : "读取 Project 失败")}
        >
          {failure !== "" && <PathError text={failure} />}
          {gone && (
            <p className="muted small">
              Project 没有归档、也没有「删除」这个操作：当它拥有的最后一个工作目录
              被移除、被别的 Project 认领，或者在本机上再也找不到时，它就自动消失了。
              它下面的 Workstream 与 Session 不会被删除 —— Project 从来不是它们的生命周期所有者。
            </p>
          )}
          {!gone && failure !== "" && (
            <p className="muted small">
              这通常只是读取失败，不代表 Project 不见了。下面的按钮会重新读一次真实状态。
            </p>
          )}
          <div className="invite">
            <button className="btn small" onClick={refresh}>重试</button>
          </div>
        </PageHeader>
      </div>
    );
  }

  const primary = detail.workstreams.filter((w) => w.is_primary);
  const related = detail.workstreams.filter((w) => !w.is_primary);
  const sessions = detail.sessions;

  return (
    <div className="main narrow">
      <PageHeader
        back="Projects"
        onBack={() => navigate({ view: "projects" })}
        title={<span style={{ overflowWrap: "anywhere" }}>{project.name}</span>}
        sub={
          <>
            这个 Project 由 NoEnding 从下面 {detail.workspace_paths.length} 个工作目录派生出来。
            你能改的只有名字；目录、成员 Workstream 与 Session 都由工作路径决定。
          </>
        }
        actions={
          <>
            <IntelligenceOnly>
              <button className="btn ghost"
                onClick={() => navigate({ view: "assistant", scope: { type: "project", id: project.id } })}>
                询问 Assistant
              </button>
            </IntelligenceOnly>
            <button className="btn"
              onClick={() => { setNameInput(project.name); setRenameError(""); setRenaming(true); }}>
              重命名
            </button>
          </>
        }
      >
        <div className="ws-detail-head-meta">
          <span className={`badge ${project.git_id ? "accent" : ""}`}>
            {project.git_id ? "由 Git 家族识别" : "普通目录家族"}
          </span>
          <span className="dot-sep" />
          <span>{project.name_customized ? "名字由你改过" : "名字自动派生"}</span>
          <span className="dot-sep" />
          <span>创建于 {timeAgo(project.created_at)}</span>
        </div>
      </PageHeader>

      <section className="rail-section" style={{ marginTop: 26 }}>
        <div className="section-label">工作目录（WorkspacePath）</div>
        {detail.workspace_paths.length === 0 && (
          <div className="l1-none">
            这个 Project 已经不再拥有任何目录 —— 它会在下一次整理时自动消失。
          </div>
        )}
        {detail.workspace_paths.map((p) => (
          <div className="list-row" key={p.id} style={{ cursor: "default" }}>
            <div className="grow">
              {/* 不用 .title：它的 nowrap + 尾部省略会把路径末段吃掉，
                  而末段正是用户用来认目录的那一段。 */}
              <div style={{ minWidth: 0, overflowWrap: "anywhere" }}>
                <PathText path={p.canonical_path} max={72} />
              </div>
              <div className="meta" style={{ whiteSpace: "normal" }}>
                首次见到 {timeAgo(p.first_seen_at)} · 最近确认 {timeAgo(p.last_seen_at)}
              </div>
            </div>
            <div className="side">
              {!p.exists && <MissingBadge />}
              <GitStateBadge state={p.git_state} kind={p.git_kind} />
            </div>
          </div>
        ))}
        <div className="small muted" style={{ marginTop: 6, maxWidth: "72ch" }}>
          一个 Project 可以有多个目录，其中一些并不是仓库。Git 与存在性都只是<b>观察结果</b>：
          丢证据不会让一条路径换 Project（方案 §1.3）。路径的身份是它的规范化字符串本身，
          不是符号链接解析后的结果。
        </div>
      </section>

      <section className="rail-section">
        <div className="section-label">Workstreams</div>
        <div className="small muted" style={{ marginBottom: 8, maxWidth: "72ch" }}>
          <b>主关联</b> = 该 Workstream 的主工作路径（position 0）落在这个 Project 上；
          <b>关联</b> = 只是经由它的其他工作路径到达。两者都是投影出来的，
          这里既不能把手工挂上、也不能摘下来。
        </div>
        {detail.workstreams.length === 0 && (
          <div className="l1-none">还没有 Workstream 经由这些目录关联进来。</div>
        )}
        {[{ title: "主关联", rows: primary }, { title: "关联", rows: related }].map((group) => (
          group.rows.length === 0 ? null : (
            <React.Fragment key={group.title}>
              <h3>{group.title} · {group.rows.length}</h3>
              {group.rows.map(({ workstream: w }) => {
                // 与 Workstream 卡片同一条规则：智能关闭时这里只出现用户自己写的描述，
                // 不展示冻结期的 Agent 摘要。规则只写在 cardSummaryLine 一处。
                const summary = cardSummaryLine(w as unknown as WorkstreamCardData, intelligenceEnabled);
                return (
                  <div key={w.id} className="list-row"
                    onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
                    <div className="grow">
                      <div className="title" title={w.title}>{w.title}</div>
                      {summary && <div className="meta">{summary}</div>}
                    </div>
                    <div className="side">
                      {w.visibility === "archived" && <span className="badge warn" title="已移入回收站；Project 与它只是投影关系">回收站</span>}
                      {w.lifecycle === "completed" && <span className="badge">已完成</span>}
                      <span>{timeAgo(w.updated_at)}</span>
                    </div>
                  </div>
                );
              })}
            </React.Fragment>
          )
        ))}
      </section>

      <section className="rail-section">
        <div className="section-label">Sessions</div>
        <div className="small muted" style={{ marginBottom: 8, maxWidth: "72ch" }}>
          这些 Session 的 cwd 正好是上面某个目录 —— 归属来自权威链
          （Session → WorkspacePath → Project），不是手工标签。
          只有旧缓存值、cwd 已经丢掉的 Session 不会出现在这里。
        </div>
        {sessions.length === 0 && (
          <div className="l1-none">这个 Project 下还没有 Session。</div>
        )}
        {sessions.slice(0, 12).map((s) => (
          <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
            <div className="grow">
              <div className="title" title={s.title ?? `${UNTITLED_SESSION} · ${s.agent_session_id}`}>
                {sessionDisplayTitle(s.title)}
              </div>
              <div className="meta mono" title={s.cwd ?? undefined}>
                {s.cwd ? cwdDisplayLabel(s.cwd, 56) : "没有记录到 cwd"}
              </div>
            </div>
            <div className="side">
              <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
              <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
            </div>
          </div>
        ))}
        {sessions.length > 12 && (
          <div className="small muted" style={{ marginTop: 6 }}>
            另有 {sessions.length - 12} 个 Session —— 到 Sessions 页查看全部。
          </div>
        )}
      </section>

      <div className="small muted" style={{ marginTop: 24, maxWidth: "72ch" }}>
        Project 会在它拥有的最后一个目录离开时自动消失；那不会删除任何 Workstream
        或 Session。想改变这里的内容，去做的是：给 Workstream 调整工作路径。
      </div>

      {renaming && (
        <Modal title="重命名 Project" onClose={() => setRenaming(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            名字只是展示信息：Project 的身份是它的 Git 家族与它拥有的路径，
            改名不会移动任何一个目录，也不会改变任何成员关系（方案 §42.6-N4：同名是允许的）。
          </p>
          <p className="muted small" style={{ marginBottom: 10 }}>
            改过之后 NoEnding 会记住「这是人起的名字」，之后的自动命名与 Git 家族合并
            都不会再覆盖它。
          </p>
          <label className="field"><span>名称</span>
            <input type="text" value={nameInput} autoFocus
              onChange={(e) => { setNameInput(e.target.value); setRenameError(""); }}
              onKeyDown={(e) => e.key === "Enter" && commitRename()} /></label>
          {renameError && (
            <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{renameError}</div>
          )}
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setRenaming(false)} disabled={renameBusy}>取消</button>
            <button className="btn primary" onClick={commitRename}
              disabled={renameBusy || nameInput.trim() === ""}>
              {renameBusy ? "保存中…" : "保存"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
