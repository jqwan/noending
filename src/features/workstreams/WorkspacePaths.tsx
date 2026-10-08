import { ellipsisPathMiddle } from "../sessions/SessionTable";
import { openPath } from "../../components/common";
import type {
  WorkspaceGitState,
  WorkstreamPathRow,
} from "../../types";

/**
 * 工作目录：一个 Workstream 的工作路径列表。
 *
 * 这一页同时展示：启动目录、Project 归属、增删路径只改这份配置（不删会话、
 * 不改已有会话的所属任务）。任何动作都要说清连带动了什么，失败留在原地说明原因。
 */

/** Git 状态徽标。`git_state` 缺失时不猜。 */
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
      <span className="badge" title={`Git 关联（${kind ?? "未知"}）`}>
        {label}
      </span>
    );
  }
  if (state === "missing") {
    return (
      <span
        className="badge warn"
        title="Git 信息暂不可用"
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
    <span className="badge warn" title="目录不存在">
      目录不存在
    </span>
  );
}

/** 完整路径：中段省略（末段才是识别信息），全串留在 title 上。点击在文件管理器中打开对应目录或定位文件。 */
export function PathText({
  path,
  max = 46,
  interactive = true,
}: {
  path: string;
  max?: number;
  interactive?: boolean;
}) {
  const text = ellipsisPathMiddle(path, max);
  if (!interactive) {
    return (
      <span className="mono" title={path} style={{ overflowWrap: "anywhere" }}>
        {text}
      </span>
    );
  }
  return (
    <span
      role="button"
      tabIndex={0}
      className="mono path-link"
      title={path}
      style={{ overflowWrap: "anywhere" }}
      onClick={(e) => {
        e.stopPropagation();
        void openPath(path);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.stopPropagation();
          e.preventDefault();
          void openPath(path);
        }
      }}
    >
      {text}
    </span>
  );
}

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
