import { useLayoutEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Modal, copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import { formatDateTime } from "./SessionTable";

/**
 * 一条会话消息：Conversation 只剩 user | assistant 两种
 * role 的 prose——compact / system / agent_message / sidechain 这些概念不再
 * 进入 UI，`who` 由调用方给出（用户 / Codex / …）。
 */
export interface SessionMessageData {
  sequence: number;
  role: "user" | "assistant";
  content: string;
  ts: string | null;
  who: string;
  /** 消息级生成溯源：仅 Assistant 有意义。 */
  provider?: string | null;
  model?: string | null;
}

/**
 * Assistant 消息头上的低干扰模型标签：
 * provider+model → "model · provider"；只有其一 → 那一个；两者皆空 → null（不占位）。
 * User 消息永远没有生成模型，调用方应根本不传。
 */
export function provenanceLabel(
  provider: string | null | undefined,
  model: string | null | undefined,
): string | null {
  const m = model?.trim();
  const p = provider?.trim();
  if (m && p) return `${m} · ${p}`;
  if (m) return m;
  if (p) return p;
  return null;
}

/**
 * 纯文本消息在列表里最多显示这么多字，全文点开弹窗看。
 * Markdown 消息不走这里——渲染结果不能按字数切，改用高度收口。
 *
 * 不在这里「展开全文」：这个流是密排的一列，就地展开会把后面的消息越推越远，
 * 而且长内容（代码、JSON、diff）在 724px 宽的消息列里折行折得很难读；
 * 弹窗给的是整块宽度 + 可滚动的一屏。
 */
const TRUNCATE_AT = 240;

/** 两种气泡：用户右、Agent 左。 */
export default function SessionMessage({ msg }: { msg: SessionMessageData }) {
  const [open, setOpen] = useState(false);
  const [clamped, setClamped] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  // 转录里的正文常带首尾空行（Codex 的尾部换行、pi 的一条前后各两个），
  // 而 `.event .body` 是 pre-wrap，不 trim 就会在上下渲染出空白行。
  const text = msg.content.trim();
  const readable = text !== "";

  const cls = msg.role === "user" ? "is-user" : "is-agent";

  // 气泡里显示的就是 Markdown 预览。判断只认内容、不认来源。
  const md = looksLikeMarkdown(text);

  // 只有真的收了角才渐隐。内容本来就短的时候挂一层渐隐，会把最后两行擦掉。
  useLayoutEffect(() => {
    const el = bodyRef.current;
    setClamped(el !== null && el.scrollHeight > el.clientHeight + 1);
  }, [text, md]);

  const stamp = `#${msg.sequence}${msg.ts ? `  ${formatDateTime(msg.ts)}` : ""}`;
  // User 消息没有生成模型——即使调用方误传也不显示。
  const prov = msg.role === "assistant" ? provenanceLabel(msg.provider, msg.model) : null;

  // 整行可点会让「悬停到哪儿」变成一条与内容无关的宽条，而点开这件事属于这条消息本身，
  // 所以热区和悬浮效果都只落在气泡上。键盘可达靠 role + tabIndex，
  // 焦点环由全局的 :focus-visible 给。
  return (
    <>
      <div className={`event ${cls}`}>
        <div className="head">
          <span className="who">{msg.who}</span>
          {prov && <span className="prov mono">{prov}</span>}
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
            ? <span className="muted">（该消息没有可读文本）</span>
            : md
              ? <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{text}</ReactMarkdown>
              : (text.length > TRUNCATE_AT ? text.slice(0, TRUNCATE_AT) + "…" : text)}
        </div>
      </div>
      {open && (
        <MessageModal who={msg.who} stamp={stamp} text={text} previewable={md} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

/** 全文弹窗：整块宽度显示，可复制；是 Markdown 的话默认就停在预览上。 */
function MessageModal({ who, stamp, text, previewable, onClose }: {
  who: string;
  stamp: string;
  text: string;
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
        <div className="msg-full">{text}</div>
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
