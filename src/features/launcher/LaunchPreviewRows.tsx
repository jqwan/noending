import type { ReactNode } from "react";
import AgentIcon from "../../components/AgentIcon";
import { openPath } from "../../components/common";
import {
  AGENT_LABELS,
  type Agent,
  type CwdResolution,
  type CwdSource,
} from "../../types";

/**
 * 启动预览的公共行：Agent、工作目录、打开方式。数据一律来自 PreparedLaunch 里
 * 冻结的意图，不在 UI 侧重算或重读 Settings——否则预览显示的不是 Agent 真正收到
 * 的东西（Preview-Launch Identity）。
 */

export function PreviewRow({
  label,
  hint,
  children,
  first,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
  first?: boolean;
}) {
  return (
    <div className="row-line" style={first ? { borderTop: 0 } : undefined}>
      <div>
        <div className="settings-row-label">{label}</div>
        {hint && <div className="settings-row-hint">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

/** Agent：本次启动实际使用的执行 Agent。 */
export function AgentRow({
  agent,
  hint,
  first,
}: {
  agent: Agent;
  hint: string;
  first?: boolean;
}) {
  return (
    <PreviewRow label="Agent" hint={hint} first={first}>
      <span className="badge accent ws-btn" style={{ gap: 6 }}>
        <AgentIcon agent={agent} />
        {AGENT_LABELS[agent]}
      </span>
    </PreviewRow>
  );
}

/** 每一层的中文名（词表说中文，「explicit」这类词不外露）。 */
const CWD_SOURCE_LABELS: Record<CwdSource, string> = {
  explicit: "你指定的目录",
  session_cwd: "会话上次的目录",
  default_workspace: "NoEnding 默认工作区",
  unresolved: "未解析",
};

export function cwdSourceLabel(resolution: CwdResolution): string {
  return CWD_SOURCE_LABELS[resolution.source];
}

/**
 * 工作目录：显示 `resolve_new_cwd` / `resolve_resume_cwd` 的解析结果，不在前端重算
 * 优先级，第一版只读。`pending` 表示 Prepare 还没回来——不许把"还没算出来"说成"没有目录"。
 * fallback 时必须说出来（来源标签 + 后端 `note`）：用户看到默认工作区路径猜不出那是降级。
 */
export function CwdRow({
  cwd,
  pending,
  resolution,
}: {
  cwd: string | null | undefined;
  pending?: boolean;
  resolution?: CwdResolution | null;
}) {
  const hint = pending
    ? "准备中…"
    : resolution
      ? `启动来源：${cwdSourceLabel(resolution)}；它不决定任务身份`
      : cwd
        ? "会话的启动目录；它不决定任务身份"
        : "未解析出目录，会话从 Agent 自身的默认位置开始";
  return (
    <>
      <PreviewRow label="工作目录" hint={hint}>
        {pending && !cwd ? (
          <span className="badge" style={{ maxWidth: 240, overflowWrap: "anywhere" }}>—</span>
        ) : cwd ? (
          <span
            role="button"
            tabIndex={0}
            className="badge path-link"
            style={{ maxWidth: 240, overflowWrap: "anywhere" }}
            title={`${cwd} · 点击在文件管理器中打开`}
            onClick={() => void openPath(cwd)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                void openPath(cwd);
              }
            }}
          >
            {cwd}
          </span>
        ) : (
          <span className="badge" style={{ maxWidth: 240, overflowWrap: "anywhere" }}>未指定</span>
        )}
      </PreviewRow>
      {resolution?.fallback && (
        <div
          className="settings-row-hint"
          style={{ color: "var(--warning)", marginTop: -8, paddingLeft: 2 }}
        >
          {resolution.note || "本次没有从这条流程通常的目录启动"}
        </div>
      )}
    </>
  );
}

/**
 * 桌面打开：`desktop_open` 非 null 时，"继续"不是开终端，而是在 Agent 的
 * 桌面应用里打开。`note` 由后端按源格式写成中文（能否定位到会话必须说出来）。
 */
export function DesktopOpenRow({ desktopOpen }: { desktopOpen: { uri: string; note: string } }) {
  return (
    <PreviewRow label="打开方式" hint="由该会话的源格式决定，已随本次预览冻结">
      <span className="badge accent">{desktopOpen.note}</span>
    </PreviewRow>
  );
}
