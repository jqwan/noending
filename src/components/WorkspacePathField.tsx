import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { PathProbe, RecentWorkspacePath } from "../types";

/**
 * 工作路径输入框（路径选择体验 v1）：一个文本输入 + 「浏览…」原生文件夹选择 +
 * 输入时的即时探测反馈 + 最近/已知路径快选。四个能力都围绕一件事——把「路径
 * 对不对、会落到哪个 Project」从提交后的裁决提前到输入时的知情。
 *
 * 探测是**顾问**：`probe_workspace_path` 只读，真正的接受判定仍发生在创建/
 * 添加时的 Rust 侧（workspace::identity 是唯一权威）。这里显示的一切都可能是
 * 过时的——所以提交永远不会被探测结果硬拦，只做提示。
 *
 * `enableProbe` / `enableRecent` 可以关掉：Session 来源（摄入目录）不是
 * WorkspacePath，Git/Project 语义不适用，只借用输入框和浏览按钮。
 */

/** 只挡明显写错的形式；「能不能当工作路径」的最终判定在 Rust 侧。 */
export const ABSOLUTE_PATH_RE = /^([~/\\]|[A-Za-z]:[\\/])/;

export function absolutePathHint(raw: string): string {
  if (raw === "") return "";
  return ABSOLUTE_PATH_RE.test(raw)
    ? ""
    : "请输入绝对路径（例如 /Users/… 、C:\\Users\\…），或写成 ~ 开头的形式。";
}

export type WorkspacePathFieldProps = {
  value: string;
  onChange: (value: string) => void;
  /** 输入框里按 Enter 时触发（追加一条 / 提交表单由调用方决定）。 */
  onSubmit?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
  /** 输入防抖后的即时探测反馈。 */
  enableProbe?: boolean;
  /** 聚焦时的最近/已知路径快选列表。 */
  enableRecent?: boolean;
  /** 原生文件夹选择按钮。 */
  enableBrowse?: boolean;
  /** 快选列表要排除的路径（通常是已经添加过的）。 */
  exclude?: string[];
  browseLabel?: string;
};

/** 输入内容的探测状态：一个可独立复用的防抖 hook（路径列表的每一行也用它）。 */
export function usePathProbe(raw: string, enabled: boolean): PathProbe | null {
  const [probe, setProbe] = useState<PathProbe | null>(null);
  useEffect(() => {
    if (!enabled) return undefined;
    const trimmed = raw.trim();
    // 形式明显不对就不发探测：绝对路径提示是即时的，不需要等 IPC。
    if (trimmed === "" || !ABSOLUTE_PATH_RE.test(trimmed)) {
      setProbe(null);
      return undefined;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .probeWorkspacePath(trimmed)
        .then((p) => { if (!cancelled) setProbe(p); })
        .catch((e) => {
          console.error(e);
          if (!cancelled) setProbe(null);
        });
    }, 300);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [raw, enabled]);
  return probe;
}

function gitLabel(kind: string | null): string {
  return kind === "main" ? "Git 主工作树" : kind === "linked" ? "Git worktree" : "Git 仓库";
}

/** 探测结果的一行反馈：绿是事实、黄是提醒，没有红色——判定权在提交那一刻。 */
export function ProbeFeedback({ probe }: { probe: PathProbe | null }) {
  if (!probe) return null;
  if (probe.status !== "ok") {
    const text =
      probe.status === "reserved"
        ? "这是 NoEnding 的自留目录，不能作为工作路径。"
        : probe.status === "home"
          ? "不能把用户主目录本身作为工作路径。"
          : "无法解析为绝对路径。";
    return <div className="badge warn" style={{ marginTop: 6, display: "block" }}>{text}</div>;
  }
  return (
    <div className="row" style={{ marginTop: 6, gap: 6, flexWrap: "wrap", alignItems: "center" }}>
      {!probe.exists && (
        <span className="badge warn" title="本机现在读不到这个目录。身份由路径字符串决定，仍然可以先作为工作路径。">
          目录不存在
        </span>
      )}
      {probe.git_state === "detected" && <span className="badge">{gitLabel(probe.git_kind)}</span>}
      {probe.git_state === "none" && <span className="badge">普通目录</span>}
      {probe.git_state === "unavailable" && (
        <span className="badge warn" title="Git 没装、超时或拒绝回答。这不影响目录本身能否作为工作路径。">
          无法检测 Git
        </span>
      )}
      {probe.project && (
        <span className="small muted">
          {probe.project.known
            ? <>归属于项目 <b>{probe.project.name ?? probe.project.id}</b></>
            : <>提交后将创建项目 <b>{probe.project.name}</b></>}
        </span>
      )}
      {probe.canonical_path && probe.raw.trim() !== probe.canonical_path && (
        <span className="mono small muted" title="NoEnding 会把输入规范化成这个路径">
          {probe.canonical_path}
        </span>
      )}
    </div>
  );
}

export default function WorkspacePathField({
  value,
  onChange,
  onSubmit,
  placeholder,
  autoFocus,
  enableProbe = true,
  enableRecent = true,
  enableBrowse = true,
  exclude = [],
  browseLabel = "浏览…",
}: WorkspacePathFieldProps) {
  const [recent, setRecent] = useState<RecentWorkspacePath[] | null>(null);
  const [listOpen, setListOpen] = useState(false);
  const [formatHint, setFormatHint] = useState("");
  const excludeRef = useRef(exclude);
  excludeRef.current = exclude;

  const probe = usePathProbe(value, enableProbe);

  const loadRecent = () => {
    if (!enableRecent) return;
    setListOpen(true);
    if (recent !== null) return;
    api
      .listRecentWorkspacePaths()
      .then(setRecent)
      .catch((e) => {
        console.error(e);
        setRecent([]);
      });
  };

  const pick = (path: string) => {
    setListOpen(false);
    setFormatHint("");
    onChange(path);
  };

  const browse = async () => {
    try {
      const picked = await open({ directory: true, multiple: false, title: "选择工作目录" });
      if (typeof picked === "string" && picked.trim() !== "") {
        setFormatHint("");
        onChange(picked.trim());
      }
    } catch (e) {
      // 没有原生对话框可用（权限/环境）时保持输入框可用，别让按钮毁掉表单。
      console.error(e);
    }
  };

  const trimmed = value.trim();
  const lowerQuery = trimmed.toLowerCase();
  const items = (recent ?? [])
    .filter((r) => !excludeRef.current.includes(r.path))
    .filter((r) => trimmed === "" || r.path.toLowerCase().includes(lowerQuery))
    .slice(0, 8);

  return (
    <div>
      <div className="row" style={{ gap: 8, alignItems: "center" }}>
        <input
          type="text"
          className="mono"
          style={{ flex: 1 }}
          value={value}
          autoFocus={autoFocus}
          placeholder={placeholder}
          onChange={(e) => { onChange(e.target.value); setFormatHint(""); }}
          onFocus={loadRecent}
          onBlur={() => setListOpen(false)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && onSubmit) {
              e.preventDefault();
              onSubmit();
            } else if (e.key === "Escape") {
              // Escape 只收起快选列表，不要顺手把整个弹窗关掉。
              if (listOpen) {
                e.stopPropagation();
                setListOpen(false);
              }
            }
          }}
        />
        {enableBrowse && (
          <button type="button" className="btn" title="在系统文件对话框里选择目录" onClick={browse}>
            {browseLabel}
          </button>
        )}
      </div>

      {listOpen && items.length > 0 && (
        <div
          className="card"
          style={{ marginTop: 6, padding: "4px 6px", maxHeight: 180, overflowY: "auto" }}
        >
          <div className="small muted" style={{ padding: "2px 4px" }}>
            {trimmed === "" ? "最近使用的工作目录" : "匹配的工作目录"}
          </div>
          {items.map((r) => (
            <button
              type="button"
              key={r.path}
              className="btn small"
              style={{
                display: "flex", width: "100%", gap: 8, alignItems: "center",
                justifyContent: "flex-start", border: "none", background: "none",
                cursor: "pointer", padding: "4px",
              }}
              // onMouseDown 抢在输入框 blur 收起列表之前完成选择。
              onMouseDown={(e) => { e.preventDefault(); pick(r.path); }}
              title={r.last_used_at ? `最近使用：${r.last_used_at}` : r.path}
            >
              <span className="mono" style={{ flex: 1, textAlign: "left", overflowWrap: "anywhere" }}>
                {r.path}
              </span>
              {r.project_name && <span className="badge">{r.project_name}</span>}
              {!r.exists && <span className="badge warn">目录不存在</span>}
              {!r.known && <span className="badge accent">未入库</span>}
            </button>
          ))}
        </div>
      )}

      {formatHint !== "" && (
        <div className="badge warn" style={{ marginTop: 6, display: "block" }}>{formatHint}</div>
      )}
      {formatHint === "" && trimmed !== "" && !ABSOLUTE_PATH_RE.test(trimmed) && (
        <div className="badge warn" style={{ marginTop: 6, display: "block" }}>
          {absolutePathHint(trimmed)}
        </div>
      )}
      <ProbeFeedback probe={probe} />
    </div>
  );
}
