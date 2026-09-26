import type { ReactNode } from "react";
import AgentIcon from "../../components/AgentIcon";
import { FIELD_LABELS } from "../settings/AgentRuntimeSettings";
import {
  AGENT_LABELS,
  type Agent,
  type AgentRuntimeOverrides,
  type CwdResolution,
  type CwdSource,
} from "../../types";

/**
 * 启动预览的公共行：Agent、Runtime、工作目录。数据一律来自 PreparedLaunch 里
 * 冻结的意图，不在 UI 侧重算或重读 Settings——否则预览显示的不是 Agent 真正收到
 * 的东西（Preview-Launch Identity）。Runtime 未设置 override 时统一说「Agent 默认值」。
 */

type Field = keyof AgentRuntimeOverrides;

/** 该 Agent 真正被 override 的字段（unsupported 字段永远为 null）。 */
function overriddenFields(agent: Agent, runtime: AgentRuntimeOverrides): Field[] {
  // `!= null`, not `!== null`: a backend-omitted field means "Agent default",
  // not "overridden with undefined" — this keeps the preview from rendering
  // `Provider undefined`.
  return (Object.keys(FIELD_LABELS[agent]) as Field[]).filter((f) => runtime[f] != null);
}

/** 「Agent 默认值」，或 override 的显式意图。 */
export function runtimeIntentText(
  agent: Agent,
  runtime: AgentRuntimeOverrides
): string {
  const overridden = overriddenFields(agent, runtime);
  if (overridden.length === 0) return "Agent 默认值";
  return overridden
    .map((f) => `${FIELD_LABELS[agent][f]} ${runtime[f]}`)
    .join(" · ");
}

export function runtimeIntentHint(
  agent: Agent,
  runtime: AgentRuntimeOverrides
): string {
  return overriddenFields(agent, runtime).length === 0
    ? "未设置 override，由 Agent 自己决定"
    : "来自设置中的 override，已随本次预览冻结";
}

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
  workstream_path: "任务的工作路径",
  default_workspace: "NoEnding 默认工作区",
  unresolved: "未解析",
};

export function cwdSourceLabel(resolution: CwdResolution): string {
  const base = CWD_SOURCE_LABELS[resolution.source];
  if (resolution.source === "workstream_path" && resolution.path_position) {
    return `${base} · 第 ${resolution.path_position + 1} 条`;
  }
  return base;
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
        <span className="badge" style={{ maxWidth: 240, overflowWrap: "anywhere" }}>
          {pending && !cwd ? "—" : cwd || "未指定"}
        </span>
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

/** Runtime：显示 PreparedLaunch 冻结的 override 意图。 */
export function RuntimeRow({
  agent,
  runtime,
}: {
  agent: Agent;
  runtime: AgentRuntimeOverrides;
}) {
  return (
    <PreviewRow label="Runtime" hint={runtimeIntentHint(agent, runtime)}>
      <span className="badge">{runtimeIntentText(agent, runtime)}</span>
    </PreviewRow>
  );
}
