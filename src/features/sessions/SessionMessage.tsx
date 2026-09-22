import { useState } from "react";
import { Modal, copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import { formatDateTime } from "./SessionTable";

export interface SessionMessageData {
  sequence: number;
  kind: string;
  text: string | null;
  ts: string | null;
  who: string;
}

/**
 * 列表里每条最多显示这么多字，全文点开弹窗看（§36.21）。
 *
 * 不在这里「展开全文」：这个流是密排的一列，就地展开会把后面的消息越推越远，
 * 而且长内容（代码、JSON、diff）在 724px 宽的消息列里折行折得很难读；
 * 弹窗给的是整块宽度 + 可滚动的一屏。
 */
const TRUNCATE_AT = 240;

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
  const [open, setOpen] = useState(false);
  // 转录里的正文常带首尾空行（Codex 的尾部换行、pi 的一条前后各两个），
  // 而 `.event .body` 是 pre-wrap，不 trim 就会在上下渲染出空白行。
  const text = (msg.text ?? "").trim();
  const readable = text !== "";
  const truncated = text.length > TRUNCATE_AT;

  const isProse = msg.kind === "user_message" || msg.kind === "assistant_message";
  const cls =
    isProse
      ? ""
      : msg.kind.startsWith("system")
        ? "tool system"
        : "tool";

  const kindLabel = EVENT_KIND_LABELS[msg.kind] ?? null;
  const who =
    msg.kind === "user_message" ? "用户"
      : msg.kind === "assistant_message" ? msg.who
      : kindLabel ?? "其他事件";
  const stamp = `#${msg.sequence}${msg.ts ? `  ${formatDateTime(msg.ts)}` : ""}`;

  return (
    <>
      <div
        className={`event ${cls}${readable ? " openable" : ""}`}
        title={readable ? "点击查看完整消息" : undefined}
        onClick={readable ? (e) => {
          // 正文要能拖选复制：点在自己选中的文字上不算「点开」，
          // 否则一次划选就会弹出弹窗。
          if (e.target instanceof HTMLElement && e.target.closest("button, a, input, select")) return;
          if ((window.getSelection()?.toString() ?? "") !== "") return;
          setOpen(true);
        } : undefined}
      >
        <div className="head">
          <span className="who" title={msg.kind}>{who}</span>
          <span className="when mono">{stamp}</span>
          {readable && (
            <button className="btn small ghost icon-button event-open"
              aria-label="查看完整消息" title="查看完整消息" onClick={() => setOpen(true)}>
              <Icon name="expand" />
            </button>
          )}
        </div>
        <div className="body">
          {readable
            ? (truncated ? text.slice(0, TRUNCATE_AT) + "…" : text)
            : <span className="muted">（该事件没有可读文本）</span>}
        </div>
      </div>
      {open && (
        <MessageModal who={who} stamp={stamp} text={text} mono={!isProse} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

/** 全文弹窗：整块宽度显示，可复制。 */
function MessageModal({ who, stamp, text, mono, onClose }: {
  who: string;
  stamp: string;
  text: string;
  mono: boolean;
  onClose: () => void;
}) {
  const copyAll = async () => {
    const ok = await copyToClipboard(text);
    showToast(ok ? "已复制全文" : "复制失败，请手动选中文字复制");
  };

  return (
    <Modal title={`${who} · ${stamp}`} wide onClose={onClose}>
      <div className={`msg-full${mono ? " mono" : ""}`}>{text}</div>
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn small" onClick={copyAll}>复制全文</button>
        <button className="btn primary" onClick={onClose}>关闭</button>
      </div>
    </Modal>
  );
}
