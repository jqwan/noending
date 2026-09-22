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

/**
 * 共用的确认 / 编辑 / 关系弹窗外壳（整体设计方案 §65）。
 *
 * Escape 与点击背板走的是同一个 `onClose`，所以调用方挂在 `onClose` 上的收尾
 * （例如 New / Resume 释放 PreparedLaunch 令牌）不会因为键盘退出而漏掉。
 *
 * 弹窗可以叠（启动 Modal 上面再开 Context 预览）。两个 Modal 的 keydown 监听
 * 都在 document 上，一次按键会同时命中，所以只让**最上面那一个**响应：
 * 自己不是最后一个 `.modal-backdrop` 时直接忽略。
 */
export function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
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
      <div className="modal" tabIndex={-1} role="dialog" aria-modal="true" aria-label={title}>
        <div className="modal-header">
          <h2>{title}</h2>
          <button type="button" className="btn ghost icon-only" aria-label="关闭弹窗" title="关闭" onClick={onClose}><Icon name="close" /></button>
        </div>
        {children}
      </div>
    </div>
  );
}
