import { useLayoutEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Modal, copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import { formatDateTime } from "./SessionTable";

export interface SessionMessageData {
  sequence: number;
  kind: string;
  text: string | null;
  ts: string | null;
  who: string;
  /** 事件 metadata 原样透传（§37.13：跨线程信封的对方身份在这里）。 */
  meta?: Record<string, unknown>;
}

/**
 * 纯文本消息在列表里最多显示这么多字，全文点开弹窗看（§36.21）。
 * Markdown 消息不走这里——渲染结果不能按字数切（§36.24），改用高度收口。
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
  // 跨线程信封（§37.13）：Codex 的子 Agent 之间互发的任务/答复。
  agent_message: "子 Agent 消息",
  unknown: "未知事件",
};

/** 统一三种 Agent 的消息视觉（整体设计方案 §43/§44）：
 * user / assistant 普通正文；tool 小型 mono block；system 弱提示。
 * 视觉差异保持克制。 */
export default function SessionMessage({ msg }: { msg: SessionMessageData }) {
  const [open, setOpen] = useState(false);
  const [clamped, setClamped] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  // 转录里的正文常带首尾空行（Codex 的尾部换行、pi 的一条前后各两个），
  // 而 `.event .body` 是 pre-wrap，不 trim 就会在上下渲染出空白行。
  const text = (msg.text ?? "").trim();
  const readable = text !== "";

  // 三种观感（§36.23）：用户右、Agent 左，都是气泡；其余是「系统噪音」，保持弱化的整行。
  const isProse = msg.kind === "user_message" || msg.kind === "assistant_message";
  const cls =
    isProse
      ? msg.kind === "user_message" ? "is-user" : "is-agent"
      : msg.kind.startsWith("system")
        ? "tool system is-tech"
        : "tool is-tech";

  // 气泡里显示的就是 Markdown 预览（§36.24）。判断只认内容、不认 kind（§36.25）：
  // 库里最像 Markdown 的恰恰是 system 事件——Codex 把整份 preamble 灌进来（实测 44KB、
  // 32 个标题、8 个围栏），而 compact 里装的是 Claude 的压缩摘要，也是正经散文。
  const md = looksLikeMarkdown(text);

  // 只有真的收了角才渐隐。内容本来就短的时候挂一层渐隐，会把最后两行擦掉。
  useLayoutEffect(() => {
    const el = bodyRef.current;
    setClamped(el !== null && el.scrollHeight > el.clientHeight + 1);
  }, [text, md]);

  const kindLabel = EVENT_KIND_LABELS[msg.kind] ?? null;
  const who =
    msg.kind === "user_message" ? "用户"
      : msg.kind === "assistant_message" ? msg.who
      : kindLabel ?? "其他事件";
  const from = counterpartOf(msg);
  const stamp = `#${msg.sequence}${msg.ts ? `  ${formatDateTime(msg.ts)}` : ""}`;

  // 整行可点会让「悬停到哪儿」变成一条与内容无关的宽条，而点开这件事属于这条消息本身，
  // 所以热区和悬浮效果都只落在气泡上（§36.24）。键盘可达靠 role + tabIndex，
  // 焦点环由全局的 :focus-visible 给。
  return (
    <>
      <div className={`event ${cls}`}>
        <div className="head">
          <span className="who" title={msg.kind}>{who}</span>
          {from && <span className="from" title={from.hint}>{from.text}</span>}
          <span className="when mono">{stamp}</span>
        </div>
        <div
          ref={bodyRef}
          className={`body${md ? " md-body" : ""}${readable ? " clickable" : ""}${clamped ? " is-clamped" : ""}`}
          role={readable ? "button" : undefined}
          tabIndex={readable ? 0 : undefined}
          title={readable ? "点击查看完整消息" : undefined}
          onClick={readable ? (e) => {
            // 正文要能拖选复制：点在自己选中的文字上不算「点开」，
            // 否则一次划选就会弹出弹窗。
            if (e.target instanceof HTMLElement && e.target.closest("button, a, input, select")) return;
            if ((window.getSelection()?.toString() ?? "") !== "") return;
            setOpen(true);
          } : undefined}
          onKeyDown={readable ? (e) => {
            if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setOpen(true); }
          } : undefined}
        >
          {!readable
            ? <span className="muted">（该事件没有可读文本）</span>
            : md
              ? <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{text}</ReactMarkdown>
              : (text.length > TRUNCATE_AT ? text.slice(0, TRUNCATE_AT) + "…" : text)}
        </div>
      </div>
      {open && (
        <MessageModal who={who} stamp={stamp} text={text} mono={!isProse} previewable={md} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

/**
 * 跨线程信封的对方（§37.13）：先说清是哪一侧——读子线程转录的人第一个问题就是
 * 「这条是启动我的任务，还是我交回去的答复」。方向由适配器从两条 agent 路径算出，
 * 解不出对方的会话 id 也照样有方向（两件事互不依赖）。
 */
const COUNTERPART_ROLE_LABELS: Record<string, string> = {
  parent: "来自父 Agent",
  child: "来自子 Agent",
  sibling: "来自同级 Agent",
};

/**
 * 显示用的两段：`text` 是「来自谁」，`hint` 是溯源信息（源自己的 message_type、
 * NoEnding 的 session id、完整 agent 路径）——对方优先显示会话标题（人读得懂），
 * 退到源里的路径；id 不是给人扫的，放在悬停提示里。两者都解不出时返回 null。
 */
function counterpartOf(msg: SessionMessageData): { text: string; hint: string } | null {
  if (msg.kind !== "agent_message") return null;
  const str = (k: string) => (typeof msg.meta?.[k] === "string" ? (msg.meta[k] as string) : null);
  const path = str("counterpart_agent_path");
  const label = str("counterpart_title") ?? path;
  const role = COUNTERPART_ROLE_LABELS[str("counterpart_role") ?? ""];
  const text = role
    ? label ? `${role} · ${label}` : role
    : label ? `来自 ${label}` : null;
  const hint = [str("message_type"), str("counterpart_session_id"), path]
    .filter((b): b is string => !!b)
    .join(" · ");
  return text ? { text, hint } : null;
}

/** 全文弹窗：整块宽度显示，可复制；是 Markdown 的话默认就停在预览上（§36.24）。 */
function MessageModal({ who, stamp, text, mono, previewable, onClose }: {
  who: string;
  stamp: string;
  text: string;
  mono: boolean;
  previewable: boolean;
  onClose: () => void;
}) {
  const [preview, setPreview] = useState(previewable);

  const copyAll = async () => {
    const ok = await copyToClipboard(text);
    showToast(ok ? "已复制全文" : "复制失败，请手动选中文字复制");
  };

  return (
    <Modal title={`${who} · ${stamp}`} wide onClose={onClose}>
      {previewable && (
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
        <button className="btn" onClick={copyAll}>复制全文</button>
        <button className="btn" onClick={onClose}>关闭</button>
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
