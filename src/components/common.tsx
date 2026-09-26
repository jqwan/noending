import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import Icon from "./Icon";

export function useRefreshSignal(cb: () => void) {
  useEffect(() => {
    const h = () => cb();
    window.addEventListener("noending:sync", h);
    return () => window.removeEventListener("noending:sync", h);
  }, [cb]);
}

export function timeAgo(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return iso;
  const s = Math.floor((Date.now() - t) / 1000);
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  return new Date(iso).toLocaleDateString();
}

export interface ContextUpdateErrorDetails {
  code: string;
  message: string;
  operationId: string | null;
}

/** Context commands reject with a structured, privacy-safe backend failure. */
export function contextUpdateErrorDetails(err: unknown): ContextUpdateErrorDetails {
  let value: unknown = err;
  if (typeof value === "string" && value.trimStart().startsWith("{")) {
    try { value = JSON.parse(value); } catch { /* keep the original value */ }
  }
  if (typeof value === "object" && value !== null) {
    const record = value as Record<string, unknown>;
    if (typeof record.code === "string" && typeof record.message === "string") {
      return {
        code: record.code,
        message: record.message,
        operationId: typeof record.operation_id === "string" ? record.operation_id : null,
      };
    }
  }
  const text = String(err);
  if (text.includes("ConcurrencyConflict")) {
    return { code: "stale_snapshot", message: "内容已变化，请重新更新", operationId: null };
  }
  if (text.includes("AiUnavailable")) {
    return { code: "ai_unavailable", message: "未配置 Assistant Agent（Settings → Agents）", operationId: null };
  }
  return { code: "update_failed", message: "更新失败，请重试", operationId: null };
}

export function contextUpdateErrorCopyText(error: ContextUpdateErrorDetails): string {
  return [
    error.message,
    `错误代码：${error.code}`,
    ...(error.operationId ? [`操作 ID：${error.operationId}`] : []),
  ].join("\n");
}

/**
 * 共用的确认 / 编辑 / 关系弹窗外壳。Escape 与点击背板走同一个 `onClose`，所以
 * 调用方挂在 `onClose` 上的收尾（例如 New / Resume 释放令牌）不会因键盘退出而漏掉。
 *
 * 弹窗可以叠：两个 Modal 的 keydown 监听都在 document 上，一次按键会同时命中，
 * 所以只让最上面那一个响应（自己不是最后一个 `.modal-backdrop` 时忽略）。
 */
export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  const backdropRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const dialog = backdropRef.current?.querySelector<HTMLElement>(".modal");
    if (!dialog?.contains(document.activeElement)) dialog?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" && e.key !== "Tab") return;
      const backdrops = document.querySelectorAll(".modal-backdrop");
      const top = backdrops[backdrops.length - 1];
      if (top !== backdropRef.current) return;
      if (e.key === "Tab") {
        const focusable = Array.from(top.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]'))
          .filter(el => !el.closest('[hidden], [aria-hidden="true"]'));
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (!first) { e.preventDefault(); return; }
        if (e.shiftKey && (document.activeElement === first || !focusable.includes(document.activeElement as HTMLElement))) {
          e.preventDefault(); last.focus();
        } else if (!e.shiftKey && (document.activeElement === last || !focusable.includes(document.activeElement as HTMLElement))) {
          e.preventDefault(); first.focus();
        }
        return;
      }
      e.stopPropagation();
      onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-backdrop" ref={backdropRef}
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal${wide ? " wide" : ""}`} tabIndex={-1} role="dialog" aria-modal="true" aria-label={title}>
        <div className="modal-header">
          <h2>{title}</h2>
          <button type="button" className="btn ghost icon-only" aria-label="关闭弹窗" title="关闭" onClick={onClose}><Icon name="close" /></button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** 剪贴板：Tauri webview 里 navigator.clipboard 通常可用（secure context），但
 *  无授权时会抛；退到隐藏 textarea + execCommand，最后返回 false 让调用方给出手动提示。 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch (e) {
    console.error(e);
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch (e) {
    console.error(e);
    return false;
  }
}
