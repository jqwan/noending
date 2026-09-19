import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { timeAgo, useRefreshSignal } from "../../components/common";
import { PathError, PathText } from "../workstreams/WorkspacePaths";
import type { Route } from "../../app/routes";
import type { Project, ProjectDetailData } from "../../types";

/**
 * Projects（方案 §22 / §42.3-M22）：v0.2 起 Project 是**派生**的——它是 NoEnding
 * 从物理工作目录（WorkspacePath）整理出来的 workspace family。用户不能新建、
 * 不能删除、也不能把 Workstream 手工挂上去，唯一可编辑的是名字。
 *
 * 所以这一页没有创建入口（页头、空态都没有），也没有删除入口；数字全部来自
 * `get_project_detail`（§11 冻结形状），而不是任何缓存成员列 ——
 * `workstreams.project_id` 在 v0.2 里是冻结的兼容列，拿它统计会算出没人指派过
 * 的关系。
 */
interface ProjectStat {
  pathCount: number;
  missingPaths: number;
  primaryWorkstreams: number;
  relatedWorkstreams: number;
  sessionCount: number;
  lastUpdate: string | null;
  /** 两条代表性目录。`workspace_paths` 由后端按 canonical_path 排序返回，
   *  Project 一侧没有「主目录」这回事（位置只在 Workstream 的路径列表里存在）。 */
  listedPaths: string[];
}

export default function ProjectsView({ navigate }: { navigate: (r: Route) => void }) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [listError, setListError] = useState("");
  const [stats, setStats] = useState<Record<string, ProjectStat>>({});
  const [detailErrors, setDetailErrors] = useState<Record<string, string>>({});
  // 每次刷新都有自己的编号：慢回来的旧详情不能盖掉新一轮的结果。
  const seqRef = useRef(0);

  const refresh = useCallback(() => {
    const seq = ++seqRef.current;
    setStats({});
    setDetailErrors({});
    api.listProjects()
      .then((ps) => {
        if (seq !== seqRef.current) return;
        setProjects(ps);
        setListError("");
        for (const p of ps) {
          api.getProjectDetail(p.id)
            .then((d) => {
              if (seq !== seqRef.current) return;
              setStats((cur) => ({ ...cur, [p.id]: statOf(d) }));
            })
            .catch((e) => {
              if (seq !== seqRef.current) return;
              setDetailErrors((cur) => ({ ...cur, [p.id]: String(e) }));
            });
        }
      })
      .catch((e) => {
        if (seq !== seqRef.current) return;
        setProjects(null);
        setListError(`读取 Project 列表失败：${String(e)}`);
      });
  }, []);

  useEffect(refresh, [refresh]);

  /**
   * 这一页是 1 + N 次读（列表 + 每个 Project 一份详情）。后台 sync 信号会成串
   * 到达，直接串起来就是几十次连读，所以把一批信号合并成一次刷新（尾沿 800ms）：
   * 数字最终会跟上真实状态，代价只是半秒。
   */
  const timerRef = useRef<number | null>(null);
  const scheduleRefresh = useCallback(() => {
    if (timerRef.current !== null) return;
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      refresh();
    }, 800);
  }, [refresh]);
  useEffect(() => () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
  }, []);
  useRefreshSignal(scheduleRefresh);

  const list = projects ?? [];

  return (
    <div className="main narrow">
      <PageHeader
        title="Projects"
        sub={
          <>
            Project 是 NoEnding 从物理工作目录（WorkspacePath）自动整理的 workspace family：
            一个 Project 可以有多个目录，其中一些并不是仓库。它由应用维护 ——
            <b>你不能新建、不能删除，也不能把 Workstream 手工挂上去，只能改名</b>；
            当它拥有的最后一个目录离开时，这个 Project 会自动消失。
          </>
        }
      />

      {listError && (
        <>
          <PathError text={listError} />
          <div className="invite">
            <button className="btn small" onClick={refresh}>重试</button>
          </div>
        </>
      )}

      {projects === null && listError === "" && <div className="muted">加载中…</div>}

      {projects !== null && projects.length === 0 && (
        <EmptyState
          title="还没有 Project"
          hint="Project 不是创建出来的，是从工作目录派生出来的：打开一个 Session，或给某条 Workstream 添加一条工作路径，它就会自动出现在这里。"
        />
      )}

      <div className="ws-stack">
        {list.map((p) => {
          const st = stats[p.id];
          const detailError = detailErrors[p.id];
          return (
            <div key={p.id} className="ws-card compact"
              onClick={() => navigate({ view: "project", projectId: p.id })}>
              <header className="ws-card-head">
                <h3 className="ws-card-title" title={p.name}>{p.name}</h3>
                <div className="ws-card-side">
                  {p.git_id && <span className="badge" title="由 Git 家族识别（同一个 common dir 的路径会收敛到一个 Project）">Git 家族</span>}
                  {st && st.missingPaths > 0 && (
                    <span className="badge warn" title={`${st.missingPaths} 个目录目前在本机上读不到。存在性只是观察，不是路径的身份。`}>
                      {st.missingPaths} 个目录不在
                    </span>
                  )}
                </div>
              </header>

              <p className="ws-card-body">
                {st === undefined
                  ? (detailError ? "目录读取失败" : "读取目录中…")
                  : st.listedPaths.length === 0
                    ? "这个 Project 目前还没有任何目录（它会在下一次整理时自动消失）"
                    : <PathText path={st.listedPaths[0]} max={60} />}
              </p>
              {st !== undefined && st.listedPaths.length > 1 && (
                <p className="ws-card-body muted">
                  <PathText path={st.listedPaths[1]} max={54} />
                  {st.pathCount > 2 ? ` —— 另有 ${st.pathCount - 2} 个目录` : ""}
                </p>
              )}

              <footer className="ws-card-meta">
                <span>
                  {detailError
                    ? "统计读取失败"
                    : st
                      ? `${st.pathCount} 个目录 · 主关联 ${st.primaryWorkstreams} · 关联 ${st.relatedWorkstreams} · ${st.sessionCount} 个 Session`
                      : "统计读取中…"}
                </span>
                <span>{st?.lastUpdate ? `最近更新 ${timeAgo(st.lastUpdate)}` : "—"}</span>
              </footer>

              {detailError && (
                <div className="ws-card-error small">
                  <PathError text={`读取 Project 详情失败：${detailError}`} />
                </div>
              )}
            </div>
          );
        })}
      </div>

      {projects !== null && projects.length > 0 && (
        <div className="small muted" style={{ marginTop: 18 }}>
          想改变一个 Project 里有什么，改的不是 Project 而是目录：给 Workstream 添加或移除
          工作路径，Project 的成员关系会跟着变。
        </div>
      )}
    </div>
  );
}

/** 详情 → 卡片上的数字。全部走 §11 冻结的 get_project_detail 投影。 */
function statOf(d: ProjectDetailData): ProjectStat {
  const missing = d.workspace_paths.filter((p) => !p.exists).length;
  const stamps = [
    ...d.workstreams.map((w) => w.workstream.updated_at),
    ...d.sessions.map((s) => s.last_activity_at ?? s.started_at ?? ""),
  ].filter((t) => t !== "");
  return {
    pathCount: d.workspace_paths.length,
    missingPaths: missing,
    primaryWorkstreams: d.workstreams.filter((w) => w.is_primary).length,
    relatedWorkstreams: d.workstreams.filter((w) => !w.is_primary).length,
    sessionCount: d.sessions.length,
    lastUpdate: stamps.sort().pop() ?? null,
    listedPaths: d.workspace_paths.slice(0, 2).map((p) => p.canonical_path),
  };
}
