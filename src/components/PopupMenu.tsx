import { useEffect, useLayoutEffect, useRef, useState, type ButtonHTMLAttributes, type HTMLAttributes, type RefObject } from "react";
import { createPortal } from "react-dom";
import Icon from "./Icon";
import "./popupMenu.css";

interface PopupMenuProps extends HTMLAttributes<HTMLDivElement> {
  /** 传入锚点时使用 Portal，避免被滚动容器或弹窗裁切。 */
  anchorRef?: RefObject<HTMLElement>;
  placement?: "auto" | "top" | "bottom";
  onDismiss?: () => void;
  focusOnOpen?: boolean;
}

/** 所有选择列表和操作菜单共用的弹层；业务模块只负责内容与开关。 */
export default function PopupMenu({ anchorRef, placement = "auto", onDismiss, focusOnOpen = true, className = "", children, onKeyDown, ...props }: PopupMenuProps) {
  const root = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number; minWidth: number; maxWidth: number; maxHeight: number } | null>(null);

  useLayoutEffect(() => {
    if (!anchorRef) return;
    const update = () => {
      const anchor = anchorRef.current;
      const menu = root.current;
      if (!anchor || !menu) return;
      const rect = anchor.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom - 12;
      const above = rect.top - 12;
      const up = placement === "top" || (placement === "auto" && below < 240 && above > below);
      const maxHeight = Math.max(40, Math.min(320, up ? above : below));
      const maxWidth = Math.max(0, Math.min(360, window.innerWidth - 16));
      const width = Math.min(maxWidth, Math.max(rect.width, menu.getBoundingClientRect().width));
      const height = Math.min(menu.scrollHeight, maxHeight);
      const next = {
        left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)),
        top: Math.max(8, up ? rect.top - height - 4 : rect.bottom + 4),
        minWidth: Math.min(rect.width, maxWidth), maxWidth, maxHeight,
      };
      setPosition(old => old && Object.keys(next).every(key => old[key as keyof typeof old] === next[key as keyof typeof next]) ? old : next);
    };
    const outside = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node) && !anchorRef.current?.contains(event.target as Node)) onDismiss?.();
    };
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    document.addEventListener("mousedown", outside);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
      document.removeEventListener("mousedown", outside);
    };
  }, [anchorRef, placement, onDismiss, children]);

  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      const trigger = anchorRef?.current ?? root.current?.parentElement?.querySelector<HTMLElement>("[aria-expanded]");
      onDismiss?.(); trigger?.focus();
    };
    document.addEventListener("keydown", escape);
    return () => document.removeEventListener("keydown", escape);
  }, [anchorRef, onDismiss]);

  const ready = !anchorRef || position !== null;
  useLayoutEffect(() => {
    if (!focusOnOpen || !ready) return;
    const items = root.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)');
    const selected = [...(items ?? [])].find(item => item.dataset.selected === "true");
    (selected ?? items?.[0])?.focus();
  }, [focusOnOpen, ready]);

  const content = (
    <div {...props} ref={root} className={`popup-menu ${className}`} role={props.role ?? "menu"}
      style={anchorRef ? { ...position, position: "fixed", visibility: position ? "visible" : "hidden", ...props.style } : props.style}
      onClick={event => { event.stopPropagation(); props.onClick?.(event); }}
      onKeyDown={event => {
        onKeyDown?.(event);
        if (event.defaultPrevented) return;
        // 让外层 Modal 继续处理 Tab，保持焦点循环；Portal 先将焦点归还触发器。
        if (event.key === "Tab") { onDismiss?.(); anchorRef?.current?.focus(); return; }
        event.stopPropagation();
        if (event.key === "Escape") {
          event.preventDefault();
          const trigger = anchorRef?.current ?? root.current?.parentElement?.querySelector<HTMLElement>("[aria-expanded]");
          onDismiss?.(); trigger?.focus(); return;
        }
        if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
        const items = [...(root.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), [role="button"]:not([aria-disabled="true"])') ?? [])];
        if (!items.length) return;
        event.preventDefault();
        const current = items.indexOf(document.activeElement as HTMLElement);
        const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
          : current < 0 ? (event.key === "ArrowUp" ? items.length - 1 : 0)
          : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
        items[next].focus();
        items[next].scrollIntoView?.({ block: "nearest" });
      }}>
      {children}
    </div>
  );
  return anchorRef ? createPortal(content, document.body) : content;
}

export function PopupMenuItem({ selected, className = "", children, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { selected?: boolean }) {
  return (
    <button type="button" {...props} className={`popup-menu-item ${className}`} data-selected={selected || undefined}
      aria-pressed={!props.role ? selected ?? props["aria-pressed"] : props["aria-pressed"]}
      aria-selected={props.role === "option" ? selected : undefined}
      aria-checked={props.role === "menuitemradio" || props.role === "menuitemcheckbox" ? selected : props["aria-checked"]}>
      {selected !== undefined && <span className="popup-menu-check" aria-hidden="true">{selected && <Icon name="check" />}</span>}
      {children}
    </button>
  );
}
