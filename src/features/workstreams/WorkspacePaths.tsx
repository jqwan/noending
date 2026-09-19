import React, { useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { ellipsisPathMiddle } from "../sessions/SessionTable";
import type {
  WorkspaceGitState,
  WorkstreamPathRow,
  WorkstreamPathSource,
} from "../../types";

/**
 * 工作路径（方案 §1.5 / §22）：一个 Workstream 的**有序**路径列表，position 0
 * 就是主工作路径 —— 角色完全由位置表达，没有第二个权威字段。
 *
 * 这一页同时是三个事实的展示位：
 *   1. 启动目录（§13：explicit cwd → WorkstreamPaths[0] → 默认 workspace）；
 *   2. Project 归属（§1.12：WorkstreamPath → WorkspacePath → Project 的投影）；
 *   3. Session 成员关系（§1.6：删一条路径会带走由它带来的 bindings）。
 *
 * 所以任何一次动作都必须说清楚它连带动了什么，失败也留在原地说明原因，
 * 绝不静默回滚（AGENTS.md：宁可保住历史与用户意图，也不图省事）。
 */

/* ---------------- 只读展示：物理观察状态 ---------------- */

/** Git 状态徽标。`git_state` 缺失时（WorkstreamPathView 就没有这一列）不猜。 */
export function GitStateBadge({ state, kind }: {
  state: WorkspaceGitState | null | undefined;
  kind?: string | null;
}) {
  if (state === "detected") {
    const label =
      kind === "main" ? "Git 主工作树"
        : kind === "linked" ? "Git worktree"
          : "Git 仓库";
    return (
      <span className="badge" title={`由 Workspace Reconcile 观察到 Git 证据（${kind ?? "unknown"}）`}>
        {label}
      </span>
    );
  }
  if (state === "missing") {
    return (
      <span
        className="badge warn"
        title="这里曾经有 Git 证据、现在读不到了。丢的是证据，不是归属：这条路径仍然属于同一个 Project（方案 §1.3）。"
      >
        Git 证据消失
      </span>
    );
  }
  if (state === "none") {
    return <span className="badge">普通目录</span>;
  }
  return null;
}

/** 存在性是观察，不是身份：目录暂时不在，路径条目仍然是合法的一条。 */
export function MissingBadge() {
  return (
    <span className="badge warn" title="本机上读不到这个目录。它仍然是这条 Workstream 的一条工作路径——身份由路径字符串决定，存在性只是观察。">
      目录不存在
    </span>
  );
}

/**
 * 路径列表条目是怎么来的（`workstream_paths.source`）。
 * 这是**来源**，不是权威：四种来源都不改变位置语义。
 */
export function pathSourceLabel(source: WorkstreamPathSource | string): string {
  switch (source) {
    case "user": return "由你添加";
    case "session": return "由 Session 绑定带入";
    case "launch": return "由启动带入";
    case "migration": return "由旧版迁移带入";
    default: return `来源：${source}`;
  }
}

/** 完整路径：中段省略（末段才是识别信息），全串留在 title 上。 */
export function PathText({ path, max = 46 }: { path: string; max?: number }) {
  return (
    <span className="mono" title={path} style={{ overflowWrap: "anywhere" }}>
      {ellipsisPathMiddle(path, max)}
    </span>
  );
}

/* ---------------- 编辑：有序工作路径列表 ---------------- */

export default function WorkstreamPathList({
  workstreamId,
  paths,
  error,
  onChanged,
}: {
  workstreamId: string;
  /** null = 还没读到；[] 是合法的「无工作路径」状态。 */
  paths: WorkstreamPathRow[] | null;
  error?: string;
  onChanged: () => void;
}) {
  const [addOpen, setAddOpen] = useState(false);
  const [addInput, setAddInput] = useState("");
  const [addFormatHint, setAddFormatHint] = useState("");
  const [removeTarget, setRemoveTarget] = useState<WorkstreamPathRow | null>(null);
  const [actionError, setActionError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  // state 是异步的，双击可能两次都读到 false；真正的单飞用 ref 守住。
  const busyRef = useRef(false);

  /**
   * 所有写操作都走这里：一次只有一个在飞，失败时把原因留在页面上并重新读
   * 一遍真实状态（失败往往意味着手里的列表已经过期），成功才给回执。
   */
  const run = async (label: string, fn: () => Promise<string | void>): Promise<boolean> => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    setActionError("");
    setNotice("");
    try {
      const receipt = await fn();
      if (receipt) setNotice(receipt);
      onChanged();
      return true;
    } catch (e) {
      setActionError(`${label}失败：${String(e)}`);
      onChanged();
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  /** §1.5：设为主路径 / 上下移动都是**整表** reorder，位置本身就是角色。 */
  const reorderTo = (orderedIds: string[], receipt: string, label: string) =>
    run(label, async () => {
      await api.reorderWorkstreamPaths(workstreamId, orderedIds);
      return receipt;
    });

  const makePrimary = (row: WorkstreamPathRow) => {
    const ids = (paths ?? []).map((p) => p.workspace_path_id);
    const next = [row.workspace_path_id, ...ids.filter((id) => id !== row.workspace_path_id)];
    return reorderTo(
      next,
      `已将 ${shortPath(row.canonical_path)} 设为主工作路径；其余路径的相对顺序保持不变。`,
      "设为主路径",
    );
  };

  const shift = (row: WorkstreamPathRow, delta: -1 | 1) => {
    const list = [...(paths ?? [])];
    const from = list.findIndex((p) => p.id === row.id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= list.length) return Promise.resolve(false);
    const ids = list.map((p) => p.workspace_path_id);
    const moved = ids.splice(from, 1)[0];
    ids.splice(to, 0, moved);
    return reorderTo(ids, `已把 ${shortPath(row.canonical_path)} 移到第 ${to + 1} 位。`, "调整顺序");
  };

  const submitAdd = () => {
    const raw = addInput.trim();
    const hint = absolutePathHint(raw);
    setAddFormatHint(hint);
    if (hint !== "" || raw === "") return;
    run("添加工作路径", async () => {
      const created = await api.addWorkstreamPath(workstreamId, raw);
      setAddOpen(false);
      setAddInput("");
      return `已添加为第 ${created.position + 1} 条工作路径。`
        + "NoEnding 不会把这个目录下已有的 Session 导进来（方案 §1.7）。";
    });
  };

  const submitRemove = () => {
    const target = removeTarget;
    if (!target) return;
    setRemoveTarget(null);
    run("移除工作路径", async () => {
      const unbound = await api.removeWorkstreamPath(workstreamId, target.id);
      const rest = (paths ?? []).filter((p) => p.id !== target.id).length;
      return `已移除 ${shortPath(target.canonical_path)}。`
        + (unbound > 0
          ? `同时把由这条路径带来的 ${unbound} 个 Session 移出本 Workstream —— Session 本身与它们的事件历史都没有被删除。`
          : "这条路径没有带走任何 Session 绑定。")
        + (rest > 0 ? " 主工作路径已由剩下第一条自动接任。" : " 本 Workstream 现在没有工作路径。");
    });
  };

  const rows = paths ?? [];

  return (
    <section className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>工作路径</div>
        {paths !== null && (
          <button className="link" onClick={() => { setAddFormatHint(""); setAddOpen(true); }}>
            添加路径
          </button>
        )}
      </div>

      <div className="small muted" style={{ marginBottom: 8 }}>
        有序列表，第 1 条是<b>主工作路径</b>：它决定新建 Session 默认在哪里启动，
        也决定这个 Workstream 出现在哪个 Project 下。路径列表不是 Workstream 的身份
        —— 它可以为空，Session 永远记住自己的 cwd。
      </div>

      {paths === null && <div className="muted small">读取工作路径…</div>}
      {error && <PathError text={error} />}

      {paths !== null && rows.length === 0 && (
        <div className="l1-none">
          还没有工作路径 —— 一条没有路径的 Workstream 完全合法。
          它不归属任何 Project，新建 Session 会从 NoEnding 的默认工作目录启动。
        </div>
      )}

      {rows.map((p) => (
        <div className="list-row" key={p.id} style={{ cursor: "default" }}>
          <div className="grow">
            {/* 路径不用 .title（那是 nowrap + 尾部省略）：一条路径最有识别度的就是
                末段，被 CSS 再截一次就等于把用户要找的东西吃掉。这里让它换行。 */}
            <div style={{ minWidth: 0, overflowWrap: "anywhere" }}>
              <PathText path={p.canonical_path} max={72} />
            </div>
            <div className="meta" style={{ whiteSpace: "normal" }}>
              {p.position === 0 ? "主工作路径" : `第 ${p.position + 1} 条`}
              {" · "}
              {pathSourceLabel(p.source)}
              {" · "}
              {p.bound_session_count === 0
                ? "没有 Session 由这条路径带来"
                : `${p.bound_session_count} 个 Session 由这条路径带来`}
              {" · "}
              {/* 只报归属、不给入口：Project 的入口在详情页的 Project 段落里，
                  一行路径不该有两个可点的 Project 链接。 */}
              <span title={`这条路径所属的 Project（由路径派生，只读）：${p.project_name ?? p.project_id}`}>
                {p.project_name ? `Project ${p.project_name}` : "Project 名称尚未同步"}
              </span>
            </div>
          </div>
          <div className="side">
            {!p.exists && <MissingBadge />}
            {p.position > 0 && (
              <button className="btn small" disabled={busy}
                title="等价于把它移动到列表第 1 位（其余顺序不变）"
                onClick={() => makePrimary(p)}>
                设为主路径
              </button>
            )}
            <button className="btn small" disabled={busy || p.position === 0}
              title={p.position === 0 ? "已经是第 1 条" : "上移一条"}
              onClick={() => shift(p, -1)}>↑</button>
            <button className="btn small" disabled={busy || p.position >= rows.length - 1}
              title={p.position >= rows.length - 1 ? "已经是最后一条" : "下移一条"}
              onClick={() => shift(p, 1)}>↓</button>
            <button className="btn small" disabled={busy} title="从本 Workstream 移除这条路径"
              onClick={() => { setActionError(""); setNotice(""); setRemoveTarget(p); }}>
              移除
            </button>
          </div>
        </div>
      ))}

      {notice && (
        <div className="small" style={{ marginTop: 8, color: "var(--success)", overflowWrap: "anywhere" }}>
          {notice}
        </div>
      )}
      {actionError && <PathError text={actionError} />}

      {addOpen && (
        <Modal title="添加工作路径" onClose={() => setAddOpen(false)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            只增加这一条路径。NoEnding 不会扫描这个目录、也不会把目录下已有的历史
            Session 导入本 Workstream（方案 §1.7）；要让某个 Session 进来，请在
            Session 里绑定它。
          </p>
          <label className="field"><span>绝对路径</span>
            <input
              type="text"
              className="mono"
              value={addInput}
              autoFocus
              placeholder="/path/to/目录 或 C:\\path\\to\\目录"
              onChange={(e) => { setAddInput(e.target.value); setAddFormatHint(""); }}
              onKeyDown={(e) => e.key === "Enter" && submitAdd()}
            /></label>
          {addFormatHint && <div className="badge warn" style={{ marginBottom: 10 }}>{addFormatHint}</div>}
          <div className="small muted" style={{ marginBottom: 10 }}>
            NoEnding 会自己把它规范化成规范路径（展开 <span className="mono">~</span>、折叠
            <span className="mono"> . </span>与<span className="mono"> .. </span>、统一分隔符），
            并且只看字符串：目录现在还不存在也可以先加进来。
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setAddOpen(false)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={submitAdd}
              disabled={busy || addInput.trim() === ""}>
              {busy ? "添加中…" : "添加"}
            </button>
          </div>
        </Modal>
      )}

      {removeTarget && (
        <Modal title="移除工作路径" onClose={() => setRemoveTarget(null)}>
          <p className="muted small" style={{ marginTop: 0 }}>
            这一步会连带改动 Workstream 的成员关系，所以先说清楚它到底动什么。
          </p>
          <div className="mono" style={{ overflowWrap: "anywhere", marginBottom: 10 }}>
            <PathText path={removeTarget.canonical_path} max={200} />
          </div>
          <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
            将同时把该路径对应的 {removeTarget.bound_session_count} 个 Session
            从当前 Workstream 移除。Session 历史不会删除。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            只移走<b>由这条路径带来</b>的绑定：手工绑定、或因 cwd 漂移而不再对应任何路径的
            绑定不会被带走（方案 §42.3-M1/M25）。
          </p>
          <p className="small muted" style={{ marginBottom: 8 }}>
            反过来，重新添加同一条路径也不会让那些 Session 回来：添加路径只是添加路径
            （方案 §1.7），要它们回来得在 Session 那边重新绑定。
          </p>
          {removeTarget.position === 0 && (rows.length > 1 ? (
            <p className="small muted" style={{ marginBottom: 8 }}>
              这是当前的主工作路径；移除后第 2 条
              （<span className="mono">{shortPath(rows[1].canonical_path)}</span>）
              自动接任，不需要你重新选择。
            </p>
          ) : (
            <p className="small muted" style={{ marginBottom: 8 }}>
              这是唯一的一条；移除后本 Workstream 没有工作路径，新建 Session
              会退回到 NoEnding 默认工作目录。
            </p>
          ))}
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setRemoveTarget(null)} disabled={busy}>取消</button>
            <button className="btn primary" onClick={submitRemove} disabled={busy}>
              {busy ? "移除中…" : "确认移除"}
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}

export function PathError({ text }: { text: string }) {
  return (
    <div className="badge warn" style={{ display: "block", marginBottom: 8, overflowWrap: "anywhere" }}>
      {text}
    </div>
  );
}

/** 只挡明显写错的形式：绝对的判定权在 Rust 侧（workspace::identity 是唯一权威）。 */
function absolutePathHint(raw: string): string {
  if (raw === "") return "";
  const absolute = /^([~/\\]|[A-Za-z]:[\\/])/.test(raw);
  return absolute
    ? ""
    : "请输入绝对路径（例如 /Users/… 、C:\\Users\\…），或写成 ~ 开头的形式。";
}

function shortPath(path: string): string {
  return ellipsisPathMiddle(path, 28);
}
