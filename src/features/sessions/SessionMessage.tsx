import { useState } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
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

  // 三种观感（§36.23）：用户右、Agent 左，都是气泡；其余是「系统噪音」，保持弱化的整行。
  const isProse = msg.kind === "user_message" || msg.kind === "assistant_message";
  const cls =
    isProse
      ? msg.kind === "user_message" ? "is-user" : "is-agent"
      : msg.kind.startsWith("system")
        ? "tool system is-tech"
        : "tool is-tech";

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

/** 全文弹窗：整块宽度显示，可复制；看起来像 Markdown 时额外给一个「预览」页签。 */
function MessageModal({ who, stamp, text, mono, onClose }: {
  who: string;
  stamp: string;
  text: string;
  mono: boolean;
  onClose: () => void;
}) {
  const [preview, setPreview] = useState(false);
  const canPreview = looksLikeMarkdown(text);

  const copyAll = async () => {
    const ok = await copyToClipboard(text);
    showToast(ok ? "已复制全文" : "复制失败，请手动选中文字复制");
  };

  return (
    <Modal title={`${who} · ${stamp}`} wide onClose={onClose}>
      {canPreview && (
        <div className="settings-seg" role="group" aria-label="显示方式" style={{ marginBottom: 12 }}>
          <button className={preview ? "" : "on"} aria-pressed={!preview} onClick={() => setPreview(false)}>原文</button>
          <button className={preview ? "on" : ""} aria-pressed={preview} onClick={() => setPreview(true)}>预览</button>
        </div>
      )}
      {preview ? (
        <div className="md-body">
          {/* 不开 rehype-raw：转录内容不可信，而这个 webview 能直接调 Tauri IPC。
              react-markdown 默认只生成 React 节点、不注入 HTML，这就够了。 */}
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{text}</ReactMarkdown>
        </div>
      ) : (
        <div className={`msg-full${mono ? " mono" : ""}`}>{text}</div>
      )}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn small" onClick={copyAll}>复制全文</button>
        <button className="btn primary" onClick={onClose}>关闭</button>
      </div>
    </Modal>
  );
}

/**
 * 看起来像 Markdown 才给预览入口。
 *
 * 纯文本没什么可预览的，硬渲染只会让人觉得东西坏了；而 Agent 的转录里本来就有大量
 * 不是 Markdown 的内容（raw 日志、ANSI、diff、半句话），所以默认永远是「原文」。
 */
export function looksLikeMarkdown(text: string): boolean {
  return /^```/m.test(text)                  // 围栏代码块
    || /^#{1,6}\s/m.test(text)               // 标题
    || /^\s*([-*+]|\d+\.)\s/m.test(text)     // 列表
    || /^>\s/m.test(text)                    // 引用
    || /^\s*\|.*\|\s*$/m.test(text)          // 表格
    || /\*\*[^*\n]+\*\*/.test(text)          // 粗体
    || /`[^`\n]+`/.test(text);               // 行内代码
}

/**
 * 两个元素被刻意退化掉（转录是我们不控制的内容）：
 *   a   —— webview 里点链接会把整个应用导航走（也没装打开外部浏览器的插件），所以只留文字；
 *   img —— 不因为一条转录就去请求远程地址（本地优先的应用不该有这种外连），显示 alt 与 URL。
 */
const MD_COMPONENTS: Components = {
  a: ({ href, children }) => <span className="md-link" title={href}>{children}</span>,
  img: ({ src, alt }) => <span className="md-img">[{alt || "图片"}] {src}</span>,
};
