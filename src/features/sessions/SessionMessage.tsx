import React, { useState } from "react";

export interface SessionMessageData {
  sequence: number;
  kind: string;
  text: string | null;
  ts: string | null;
  who: string;
}

/**
 * 统一三种 Agent 的消息视觉（整体设计方案 §43/§44）：
 * user / assistant 普通正文；tool 小型 mono block；system 弱提示。
 * 视觉差异保持克制。
 */
export default function SessionMessage({ msg }: { msg: SessionMessageData }) {
  const [expanded, setExpanded] = useState(false);
  const text = msg.text ?? "";
  const long = text.length > 420;

  const cls =
    msg.kind === "user_message" || msg.kind === "assistant_message"
      ? ""
      : msg.kind.startsWith("system")
        ? "tool system"
        : "tool";

  const who =
    msg.kind === "user_message" ? "You"
      : msg.kind === "assistant_message" ? msg.who
      : msg.kind;

  return (
    <div className={`event ${cls}`} key={msg.sequence}>
      <div className="head">
        <span className="who">{who}</span>
        <span className="when mono">
          #{msg.sequence}
          {msg.ts ? `  ${new Date(msg.ts).toLocaleString()}` : ""}
        </span>
      </div>
      <div className="body">{long && !expanded ? text.slice(0, 420) + "…" : text}</div>
      {long && (
        <button className="link" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "收起" : "展开全文"}
        </button>
      )}
    </div>
  );
}
