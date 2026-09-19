import type { ReactNode } from "react";
import AgentIcon from "../../components/AgentIcon";
import { FIELD_LABELS } from "../settings/AgentRuntimeSettings";
import {
  AGENT_LABELS,
  type Agent,
  type AgentRuntimeOverrides,
  type ContextDeliveryLevel,
  type CwdResolution,
  type CwdSource,
} from "../../types";

/**
 * 启动预览的公共行（方案 §15「Preview 仅展示」）：Agent、Runtime、工作目录。
 *
 * 这些数据一律来自 PreparedLaunch 里冻结的意图，而不是在 UI 侧重算或重新读取
 * Settings——否则预览显示的就不是 Agent 真正收到的东西（Preview-Launch
 * Identity）。Runtime 未设置 override 时统一说「Agent 默认值」（§2 词表）：
 * 默认值属于 Agent，NoEnding 只拥有 override。
 */

type Field = keyof AgentRuntimeOverrides;

/** 该 Agent 真正被 override 的字段（unsupported 字段永远为 null）。 */
function overriddenFields(agent: Agent, runtime: AgentRuntimeOverrides): Field[] {
  // `!= null`, not `!== null`: a field the backend ever omits is "Agent
  // default", not "overridden with the value undefined" — the looser compare is
  // what keeps the preview from rendering `Provider undefined`.
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

/** 实验区文案：四级 delivery 的中文名（§2.1，只在 Delivery 未关闭时出现）。 */
export function deliveryLevelLabel(level: ContextDeliveryLevel): string {
  switch (level) {
    case "off":
      return "关闭";
    case "compact":
      return "精简";
    case "balanced":
      return "均衡";
    case "detailed":
      return "详细";
  }
}

export function PreviewRow({
  label,
  hint,
  children,
  first,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
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

/** §13 每一层的中文名。「用户选择的目录」而非「explicit」——词表 §2 说中文。 */
const CWD_SOURCE_LABELS: Record<CwdSource, string> = {
  explicit: "你指定的目录",
  session_cwd: "Session 上次的目录",
  workstream_path: "Workstream 的工作路径",
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
 * 工作目录：显示 `resolve_new_cwd` / `resolve_resume_cwd` 的解析结果，不在前端
 * 重算优先级，第一版只读（开放编辑需要后端新增入参，见 §15 默认处理第 4 条）。
 * `pending` 表示 Prepare 还没回来——此时不许把"还没算出来"说成"没有目录"。
 *
 * 发生 fallback 时必须说出来（§13）：来源标签 + 后端的 `note`，而不是只显示一个
 * 目录字符串——用户看到 `/Users/me/.noending/workspace` 猜不出那是降级结果。
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
      ? `启动来源：${cwdSourceLabel(resolution)}；它不决定 Workstream 身份`
      : cwd
        ? "Session 的启动目录；它不决定 Workstream 身份"
        : "未解析出目录，Session 从 Agent 自身的默认位置开始";
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
