import { ellipsisPathMiddle } from "../sessions/SessionTable";
import { openPath } from "../../components/common";
import type {
  WorkspaceGitState,
} from "../../types";

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

export function PathError({ text }: { text: string }) {
  return (
    <div className="badge warn" style={{ display: "block", marginBottom: 8, overflowWrap: "anywhere" }}>
      {text}
    </div>
  );
}
