import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal, openPath, submitsOnEnter } from "../../components/common";
import type {
  CreateWorkstreamReport,
  ProjectCardData,
  Workstream,
  WorkstreamPathRow,
} from "../../types";

/** 任务表单弹窗：新建与编辑共用，统一按项目维度关联（支持关联多个项目）。 */

type PathOutcome = CreateWorkstreamReport["paths"][number];

/** `created` = 任务刚建好；`saved` = 改动已落盘。只有文案不同。 */
type PathReport = {
  outcome: "created" | "saved";
  workstream: Workstream;
  paths: PathOutcome[];
};

function rejected(raw: string, reason: string): PathOutcome {
  return {
    raw,
    accepted: false,
    canonical_path: null,
    position: null,
    project_name: null,
    reason,
  };
}

export default function WorkstreamFormModal({
  onClose,
  onCreated,
  onSaved,
  workstream,
  paths,
  initialProjectId,
}: {
  onClose: () => void;
  /** 创建成功后回调（含「路径没全接受、用户已知情」的收尾）。 */
  onCreated?: (w: Workstream) => void;
  /** 编辑保存后回调；调用方据此重新读一遍投影。 */
  onSaved?: () => void;
  /** 传了就是编辑模式。 */
  workstream?: Workstream;
  /** 编辑模式下当前任务的有序工作目录；创建模式不传。 */
  paths?: WorkstreamPathRow[];
  /** 可选：新建模式下初始选中的项目 ID。 */
  initialProjectId?: string;
}) {
  const editing = workstream !== undefined;
  // 取打开那一刻的快照，不跟着后台刷新的 props 走：否则别处刚附上的路径会被
  // 当成「用户拿掉了它」而在保存时被静默移除。
  const [existing] = useState<WorkstreamPathRow[]>(() => paths ?? []);
  const [title, setTitle] = useState(workstream?.title ?? "");
  const [desc, setDesc] = useState(workstream?.description ?? "");

  // 项目选择状态：支持关联多个项目
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [defaultWorkspace, setDefaultWorkspace] = useState<string>("");
  const [newProjects, setNewProjects] = useState<
    Array<{ id: string; name: string; paths: string[] }>
  >([]);

  const [selectedProjectIds, setSelectedProjectIds] = useState<string[]>(() => {
    if (editing) {
      const ids: string[] = [];
      for (const p of paths ?? []) {
        const pid =
          p.project_id ||
          (p.project_name === "NoEnding Workspace" ? "default" : "") ||
          (p.canonical_path ? `path:${p.canonical_path}` : "");
        if (pid && !ids.includes(pid)) {
          ids.push(pid);
        }
      }
      return ids;
    }
    return [initialProjectId && initialProjectId !== "none" ? initialProjectId : "default"];
  });
  const [browsingProject, setBrowsingProject] = useState(false);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [report, setReport] = useState<PathReport | null>(null);
  const busyRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    api
      .listProjectCards()
      .then((cards) => {
        if (!cancelled) setProjects(cards);
      })
      .catch(console.error);
    api
      .getWorkspaceSettings?.()
      ?.then((s) => {
        if (!cancelled && s?.default_workspace) setDefaultWorkspace(s.default_workspace);
      })
      ?.catch(console.error);
    return () => {
      cancelled = true;
    };
  }, []);

  const defaultProject = projects.find(
    (p) =>
      p.name === "NoEnding Workspace" ||
      (defaultWorkspace && p.search_paths?.includes(defaultWorkspace))
  );
  const defaultProjectId = defaultProject?.id ?? "default";

  const isDefaultProj = useCallback(
    (pid: string) => pid === "default" || pid === defaultProjectId || pid === defaultProject?.id,
    [defaultProjectId, defaultProject]
  );

  const getProjectPaths = useCallback(
    (pid: string): string[] => {
      if (isDefaultProj(pid)) {
        if (defaultProject?.search_paths?.length) return defaultProject.search_paths;
        if (defaultProject?.representative_paths?.length) return defaultProject.representative_paths;
        if (defaultWorkspace) return [defaultWorkspace];
        const fromExisting = existing
          .filter(
            (p) =>
              p.project_name === "NoEnding Workspace" ||
              (defaultWorkspace && p.canonical_path === defaultWorkspace)
          )
          .map((p) => p.canonical_path);
        if (fromExisting.length > 0) return fromExisting;
        return [];
      }
      const fromProjects = projects.find((p) => p.id === pid);
      if (fromProjects?.search_paths?.length) return fromProjects.search_paths;
      if (fromProjects?.representative_paths?.length) return fromProjects.representative_paths;
      const fromNew = newProjects.find((p) => p.id === pid);
      if (fromNew?.paths?.length) return fromNew.paths;
      const fromExisting = existing
        .filter((p) => p.project_id === pid || `path:${p.canonical_path}` === pid)
        .map((p) => p.canonical_path);
      if (fromExisting.length > 0) return fromExisting;
      return [];
    },
    [isDefaultProj, defaultProject, defaultWorkspace, projects, newProjects, existing]
  );

  const selectedProjectPaths = useMemo(() => {
    const result: string[] = [];
    for (const pid of selectedProjectIds) {
      for (const p of getProjectPaths(pid)) {
        if (!result.includes(p)) {
          result.push(p);
        }
      }
    }
    return result;
  }, [selectedProjectIds, getProjectPaths]);

  const handleAddProject = (pid: string) => {
    setSelectedProjectIds((prev) => {
      if (isDefaultProj(pid)) {
        if (prev.some(isDefaultProj)) return prev;
        return [...prev, pid];
      }
      if (prev.includes(pid)) return prev;
      return [...prev, pid];
    });
  };

  const handleRemoveProject = (pid: string) => {
    setSelectedProjectIds((prev) => {
      if (isDefaultProj(pid)) {
        return prev.filter((id) => !isDefaultProj(id));
      }
      return prev.filter((id) => id !== pid);
    });
  };

  const handleAddNewProject = async () => {
    setBrowsingProject(true);
    setError("");
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择项目目录",
      });
      if (!picked) return;
      const pickedPath = Array.isArray(picked) ? picked[0] : picked;
      if (!pickedPath) return;

      const probe = await api.probeWorkspacePath(pickedPath);
      if (probe) {
        if (probe.status === "reserved") {
          setError("该目录为 NoEnding 自留目录，不能作为项目目录");
          return;
        }
        if (probe.status === "home") {
          setError("不能是用户主目录本身");
          return;
        }
        if (probe.status === "unresolvable") {
          setError("无法解析为绝对路径");
          return;
        }
      }

      if (probe?.project?.known && probe.project.id) {
        // 若选择的目录被归为一个已有项目，就加入关联项目
        const existingId = probe.project.id;
        handleAddProject(existingId);
        if (!projects.some((p) => p.id === existingId)) {
          const fresh = await api.listProjectCards();
          setProjects(fresh);
        }
      } else {
        // 以这个目录生成项目并加入关联项目
        const projName =
          probe?.project?.name ||
          pickedPath.split(/[\\/]/).filter(Boolean).pop() ||
          "新项目";
        const canonicalPath = probe?.canonical_path || pickedPath;
        const newId = `new:${canonicalPath}`;
        setNewProjects((prev) => {
          const filtered = prev.filter((p) => p.id !== newId);
          return [
            ...filtered,
            { id: newId, name: projName, paths: [canonicalPath] },
          ];
        });
        handleAddProject(newId);
      }
    } catch (e) {
      console.error(e);
      setError(`选择项目目录失败：${String(e)}`);
    } finally {
      setBrowsingProject(false);
    }
  };

  const titleTrimmed = title.trim();
  const descTrimmed = desc.trim();
  const existingRaws = existing.map((p) => p.canonical_path);
  const draftRaws = selectedProjectPaths;
  const pathsUntouched =
    draftRaws.length === existingRaws.length &&
    draftRaws.every((raw, i) => raw === existingRaws[i]);
  /** 保存后会消失的已有路径。路径与 Session 归属不再联动，所以没有连带计数。 */
  const keptRaw = new Set(draftRaws);
  const dropping = existing.filter((p) => !keptRaw.has(p.canonical_path));

  const dirty = editing
    ? titleTrimmed !== workstream.title ||
      descTrimmed !== (workstream.description ?? "") ||
      !pathsUntouched
    : titleTrimmed !== "";

  const create = async () => {
    if (titleTrimmed === "" || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      const pathsToSubmit = selectedProjectPaths;
      const r = await api.createWorkstream(
        titleTrimmed,
        desc,
        pathsToSubmit,
      );
      if (r.paths.some((p) => !p.accepted)) {
        setReport({ outcome: "created", workstream: r.workstream, paths: r.paths });
        return;
      }
      onCreated?.(r.workstream);
      onClose();
    } catch (e) {
      // 失败时留在弹窗里、把原因说出来：静默关闭会让用户以为已经建好了。
      console.error(e);
      setError(String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /** 工作目录按「认领 → 解析 → 移除」落地：草稿里与已有行 canonical_path 逐字相同的
   *  直接认领它的 id，只有新加的路径才交给 `addWorkstreamPath`；移除只针对真的从列表里
   *  拿掉的路径——某一次解析失败不是删除的理由。 */
  const save = async () => {
    if (!workstream || busyRef.current || !dirty) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      // description 在这里 trim：整对象写不做规范化，而创建路径是 trim 后落库的。
      if (titleTrimmed !== workstream.title || descTrimmed !== (workstream.description ?? "")) {
        await api.updateWorkstream({
          ...workstream,
          title: titleTrimmed,
          description: descTrimmed,
        });
      }
      if (!pathsUntouched) {
        const unconsumed = new Map(existing.map((p) => [p.canonical_path, p]));
        const orderedIds: string[] = [];
        const failures: PathOutcome[] = [];
        for (const raw of draftRaws) {
          const claimed = unconsumed.get(raw);
          if (claimed) {
            unconsumed.delete(raw);
            orderedIds.push(claimed.workspace_path_id);
            continue;
          }
          try {
            const row = await api.addWorkstreamPath(workstream.id, raw);
            // 两种拼写、同一个目录：保留先出现的那一条，与创建时的处理一致。
            if (orderedIds.includes(row.workspace_path_id)) {
              failures.push(rejected(raw, "与前面一条指向同一目录"));
              continue;
            }
            orderedIds.push(row.workspace_path_id);
          } catch (e) {
            failures.push(rejected(raw, String(e)));
          }
        }
        for (const p of unconsumed.values()) {
          await api.removeWorkstreamPath(workstream.id, p.id);
        }
        if (orderedIds.length > 0) {
          await api.reorderWorkstreamPaths(workstream.id, orderedIds);
        }
        if (failures.length > 0) {
          setReport({ outcome: "saved", workstream, paths: failures });
          return;
        }
      }
      onSaved?.();
      onClose();
    } catch (e) {
      console.error(e);
      setError(String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const submit = () => (editing ? save() : create());

  const leave = () => {
    if (report) {
      if (report.outcome === "created") onCreated?.(report.workstream);
      else onSaved?.();
    }
    onClose();
  };

  if (report) {
    const created = report.outcome === "created";
    const accepted = report.paths.filter((p) => p.accepted).length;
    const allRejected = report.paths.length > 0 && accepted === 0;
    return (
      <Modal
        title={
          created
            ? allRejected
              ? "任务已创建（没有工作目录）"
              : "任务已创建（部分路径未接受）"
            : "任务已保存（部分路径未生效）"
        }
        onClose={leave}
      >
        <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
          {created ? (
            <>
              <b>{report.workstream.title}</b> 已经建好了。提交的 {report.paths.length} 条路径里，
              {accepted} 条被接受{allRejected ? "，0 条被接受" : ""}。
            </>
          ) : (
            <>
              <b>{report.workstream.title}</b> 的标题、描述与其它工作目录已经保存。提交的{" "}
              {report.paths.length} 条路径里，{report.paths.length - accepted} 条没有生效。
            </>
          )}
        </p>
        <div style={{ marginBottom: 10 }}>
          {report.paths.map((p, i) => (
            <div key={`${p.raw}-${i}`} className="list-row" style={{ cursor: "default" }}>
              <div className="grow" style={{ minWidth: 0 }}>
                <span
                  role="button"
                  tabIndex={0}
                  className="mono path-link"
                  style={{ overflowWrap: "anywhere" }}
                  title={`${p.raw} · 点击在文件管理器中打开`}
                  onClick={() => void openPath(p.raw)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      void openPath(p.raw);
                    }
                  }}
                >
                  {p.raw}
                </span>
              </div>
              <div className="side">
                {p.accepted ? (
                  <span className="badge accent">
                    第 {(p.position ?? 0) + 1} 条{p.project_name ? ` · 项目 ${p.project_name}` : ""}
                  </span>
                ) : (
                  <span className="badge warn" style={{ maxWidth: 280, overflowWrap: "anywhere" }}>
                    {p.reason ?? "未接受"}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
        <p className="small muted" style={{ marginBottom: 12 }}>
          {created
            ? allRejected
              ? "没有工作目录的任务依然有效：新建会话会从 NoEnding 的默认工作目录启动，也不归属任何项目。你可以在详情页随时添加工作路径。"
              : "被拒绝的路径不影响其余路径：任务按提交顺序带上了被接受的部分。"
            : "没生效的路径不影响其余改动：其余工作目录、标题与描述都已经保存。可以在列表里改好这条路径后重新编辑。"}
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn primary" onClick={leave}>知道了</button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal title={editing ? "编辑任务" : "新建任务"} onClose={onClose}>
      <label className="field">
        <span>标题</span>
        <input
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          autoFocus
          placeholder="例如：接口设计 / 行程规划 / 预算整理"
          onKeyDown={(e) => submitsOnEnter(e) && submit()}
        />
      </label>
      <label className="field">
        <span>描述（可选）</span>
        <textarea
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
          placeholder="简要记录任务目标、注意事项或范围…"
          rows={3}
        />
      </label>
      <div className="field">
        <span style={{ display: "block", fontSize: 12, color: "var(--text-muted)", marginBottom: 6 }}>
          关联项目（可选，可多选）
        </span>

        {selectedProjectIds.length > 0 ? (
          <div className="workstream-project-chips">
            {selectedProjectIds.map((pid) => {
              const isDefault = isDefaultProj(pid);
              const proj = isDefault
                ? defaultProject
                : projects.find((p) => p.id === pid) ?? newProjects.find((p) => p.id === pid);
              const name = isDefault
                ? "NoEnding Workspace"
                : proj?.name ??
                  existing.find((p) => p.project_id === pid || `path:${p.canonical_path}` === pid)?.project_name ??
                  existing.find((p) => `path:${p.canonical_path}` === pid)?.canonical_path ??
                  pid;
              return (
                <div key={pid} className="workstream-project-chip">
                  <Icon name="folder" />
                  <span className="truncate" title={name}>
                    {name}
                  </span>
                  <button
                    type="button"
                    className="workstream-project-chip-remove"
                    aria-label={`移除项目 ${name}`}
                    title={`移除项目 ${name}`}
                    onClick={() => handleRemoveProject(pid)}
                    disabled={busy || browsingProject}
                  >
                    <Icon name="close" />
                  </button>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="small muted" style={{ marginBottom: 8 }}>
            暂未关联任何项目
          </div>
        )}

        <div className="row" style={{ gap: 8, alignItems: "center" }}>
          <select
            style={{ flex: 1, minWidth: 0 }}
            value=""
            onChange={(e) => {
              if (e.target.value) {
                handleAddProject(e.target.value);
              }
            }}
            disabled={busy || browsingProject}
            aria-label="关联项目选择"
          >
            <option value="">+ 添加关联项目…</option>
            {!selectedProjectIds.some(isDefaultProj) && (
              <option value={defaultProjectId}>NoEnding Workspace</option>
            )}
            {projects
              .filter(
                (p) =>
                  !selectedProjectIds.includes(p.id) &&
                  !isDefaultProj(p.id) &&
                  p.name !== "NoEnding Workspace"
              )
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            {newProjects
              .filter((p) => !selectedProjectIds.includes(p.id))
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
          </select>
          <button
            type="button"
            className="btn small"
            style={{ flexShrink: 0, display: "inline-flex", alignItems: "center", gap: 4 }}
            onClick={handleAddNewProject}
            disabled={busy || browsingProject}
          >
            <Icon name="plus" />
            <span>{browsingProject ? "选择中…" : "新增项目"}</span>
          </button>
        </div>
      </div>
      {editing && dropping.length > 0 && (
        <div className="badge warn" style={{ display: "block", marginBottom: 10, overflowWrap: "anywhere" }}>
          保存后会从当前任务移除 {dropping.length} 条工作目录。不会删除会话，也不会修改已有会话的所属任务。
        </div>
      )}
      {error && (
        <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{error}</div>
      )}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 18, gap: 10 }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={busy || !dirty} onClick={submit}>
          {busy
            ? editing ? "保存中…" : "创建中…"
            : editing ? "保存" : "创建"}
        </button>
      </div>
    </Modal>
  );
}
