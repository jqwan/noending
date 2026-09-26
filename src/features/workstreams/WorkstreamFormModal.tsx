import { useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import PathListEditor, { type PathEntryDraft } from "./PathListEditor";
import type { CreateWorkstreamReport, Workstream, WorkstreamPathRow } from "../../types";

/** 任务表单弹窗：新建与编辑共用，传 `workstream` 即编辑模式。
 *  Project 由工作目录派生，用户既不能挑也不能造；被拒绝的路径逐条说破，不静默丢掉。 */

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
}) {
  const editing = workstream !== undefined;
  // 取打开那一刻的快照，不跟着后台刷新的 props 走：否则别处刚附上的路径会被
  // 当成「用户拿掉了它」而在保存时被静默移除。
  const [existing] = useState<WorkstreamPathRow[]>(() => paths ?? []);
  const [title, setTitle] = useState(workstream?.title ?? "");
  const [desc, setDesc] = useState(workstream?.description ?? "");
  // 草稿行是只读展示，用户只能新增 / 移除 / 换序，改不了已有条目的拼写。
  const [entries, setEntries] = useState<PathEntryDraft[]>(
    () => existing.map((p) => ({ raw: p.canonical_path })),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [report, setReport] = useState<PathReport | null>(null);
  const busyRef = useRef(false);

  const titleTrimmed = title.trim();
  const descTrimmed = desc.trim();
  const existingRaws = existing.map((p) => p.canonical_path);
  const draftRaws = entries.map((e) => e.raw.trim()).filter((raw) => raw !== "");
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
      const r = await api.createWorkstream(
        titleTrimmed,
        desc,
        entries.map((e) => e.raw),
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
                <span className="mono" style={{ overflowWrap: "anywhere" }} title={p.raw}>
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
      <label className="field"><span>标题</span>
        <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
          placeholder="例如：接口设计 / 行程规划 / 预算整理"
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229 && submit()} /></label>
      <label className="field"><span>描述（可选）</span>
        <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
      <div className="field"><span>工作目录（可选，可多条）</span>
        <PathListEditor entries={entries} onChange={setEntries} /></div>
      {editing && dropping.length > 0 && (
        <div className="badge warn" style={{ display: "block", marginBottom: 10, overflowWrap: "anywhere" }}>
          保存后会从当前任务移除 {dropping.length} 条工作目录。不会删除会话，也不会修改已有会话的所属任务。
        </div>
      )}
      {error && (
        <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{error}</div>
      )}
      <div className="row" style={{ justifyContent: "flex-end" }}>
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
