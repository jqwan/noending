import { ellipsisPathMiddle } from "../sessions/SessionTable";
import type {
  WorkspaceGitState,
  WorkstreamPathRow,
  WorkstreamPathSource,
} from "../../types";

/**
 * 工作目录（方案 §1.5 / §22）：一个 Workstream 的**有序**路径列表，position 0
 * 就是主工作目录 —— 角色完全由位置表达，没有第二个权威字段。
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
        title="Git 信息暂不可用，项目归属不变。"
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
    <span className="badge warn" title="本机上读不到这个目录。它仍然是这项任务的一条工作目录——身份由路径字符串决定，存在性只是观察。">
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
    case "session": return "由会话绑定带入";
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

/* ---------------- 只读展示：有序工作目录列表 ---------------- */

/* ---------------- 只读展示：有序工作目录列表 ---------------- */

/** 只读：增 / 删 / 换序都在「••• → 编辑任务」里。 */
export default function WorkstreamPathList({
  paths,
  error,
}: {
  /** null = 还没读到；[] 是合法的「无工作目录」状态。 */
  paths: WorkstreamPathRow[] | null;
  error?: string;
}) {
  const rows = paths ?? [];

  return (
    <section className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>工作目录</div>
      </div>

      {paths === null && <div className="muted small">读取工作目录…</div>}
      {error && <PathError text={error} />}

      {paths !== null && rows.length === 0 && (
        <div className="l1-none">
          未设置工作目录，新会话使用默认目录。
        </div>
      )}

      {rows.map((p) => (
        <div className="list-row" key={p.id} style={{ cursor: "default" }}>
          <div className="grow" style={{ minWidth: 0 }}>
            {/* 路径不用 .title（那是 nowrap + 尾部省略）：一条路径最有识别度的就是
                末段，被 CSS 再截一次就等于把用户要找的东西吃掉。这里让它换行。 */}
            <div style={{ overflowWrap: "anywhere" }}>
              <PathText path={p.canonical_path} max={72} />
            </div>
          </div>
          {/* 顺序就是角色（§1.5）：第 1 条即主工作目录。 */}
          {p.position === 0 && (
            <span className="badge accent" title="新建会话默认从这里启动，也决定项目归属">
              主目录
            </span>
          )}
        </div>
      ))}
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
