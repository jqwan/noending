import { useState } from "react";
import { formatDateTime } from "./SessionTable";

export interface SessionMessageData {
  sequence: number;
  kind: string;
  text: string | null;
  ts: string | null;
  who: string;
}

/**
 * 技术性事件 kind 的中文显示名。工具事件已从摄入里撤下（方案 §36.11），历史行也已清除，
 * 所以这里不再有 tool_call / tool_result。
 * user_message / assistant_message 由调用方给的 `who` 承担（用户 / Codex / …）。
 * 原始 kind 保留在 title 提示里，报障时仍能对上转录证据（AGENTS.md: Provenance Fidelity）。
 */
const EVENT_KIND_LABELS: Record<string, string> = {
  system: "系统",
  compact: "上下文压缩",
  unknown: "未知事件",
};

/** 统一三种 Agent 的消息视觉（整体设计方案 §43/§44）：
 * user / assistant 普通正文；tool 小型 mono block；system 弱提示。
 * 视觉差异保持克制。 */
export default function SessionMessage({ msg }: { msg: SessionMessageData }) {
  const [expanded, setExpanded] = useState(false);
  // 转录里的正文常带首尾空行（Codex 的尾部换行、pi 的一条前后各两个），
  // 而 `.event .body` 是 pre-wrap，不 trim 就会在上下渲染出空白行。
  const text = (msg.text ?? "").trim();
  const long = text.length > 420;

  const cls =
    msg.kind === "user_message" || msg.kind === "assistant_message"
      ? ""
      : msg.kind.startsWith("system")
        ? "tool system"
        : "tool";

  const kindLabel = EVENT_KIND_LABELS[msg.kind] ?? null;
  const who =
    msg.kind === "user_message" ? "用户"
      : msg.kind === "assistant_message" ? msg.who
      : kindLabel ?? "其他事件";

  return (
    <div className={`event ${cls}`} key={msg.sequence}>
      <div className="head">
        <span className="who" title={msg.kind}>{who}</span>
        <span className="when mono">
          #{msg.sequence}
          {/* 不用 toLocaleString()：webview 语言不一定是中文，会漏出 AM/PM（§1.2） */}
          {msg.ts ? `  ${formatDateTime(msg.ts)}` : ""}
        </span>
      </div>
      <div className="body">
        {text === ""
          ? <span className="muted">（该事件没有可读文本）</span>
          : long && !expanded ? text.slice(0, 420) + "…" : text}
      </div>
      {long && (
        <button className="link" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "收起" : "展开全文"}
        </button>
      )}
    </div>
  );
}
