import { createContext, useContext, useId, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { booleanPreference, useViewPreference } from "../hooks/useViewState";
import Icon from "./Icon";

type Panel = {
  kind: string;
  open: boolean;
  id: string;
  toggle: () => void;
  close: () => void;
  hint?: string;
  button: React.RefObject<HTMLButtonElement>;
  element: React.RefObject<HTMLElement>;
};
const PanelContext = createContext<Panel | null>(null);
const FooterContext = createContext<HTMLElement | null>(null);

export function DetailPanelProvider({ kind, children, hint }: { kind: string; children: ReactNode; hint?: string }) {
  const [open, setOpen] = useViewPreference(`details.${kind}.panel`, !(window.matchMedia?.("(max-width: 900px)").matches ?? false), booleanPreference);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const element = useRef<HTMLElement>(null);
  const close = () => {
    if (element.current?.contains(document.activeElement)) button.current?.focus();
    setOpen(false);
  };
  return <PanelContext.Provider value={{ kind, open, id, button, element, close, hint, toggle: () => open ? close() : setOpen(true) }}>{children}</PanelContext.Provider>;
}

export function useDetailPageKind() {
  return useContext(PanelContext)?.kind ?? "detail";
}

/** PageHeader 自动提供统一入口，也可在专属工具栏中单独使用。 */
export function DetailPanelToggle() {
  const panel = useContext(PanelContext);
  if (!panel) return null;
  return <button ref={panel.button} type="button" className={`btn ghost icon-button${panel.open ? " on" : ""}`}
    aria-label={panel.open ? "收起详情栏" : "展开详情栏"} title={`${panel.open ? "收起详情栏" : "展开详情栏"}${panel.hint ? ` · ${panel.hint}` : ""}`}
    aria-expanded={panel.open} aria-controls={panel.id} onClick={panel.toggle}><Icon name="panelRight" /></button>;
}

export function DetailSidePanel({ children, label = "详情栏" }: { children: ReactNode; label?: string }) {
  const panel = useContext(PanelContext);
  const [footer, setFooter] = useState<HTMLDivElement | null>(null);
  if (!panel) throw new Error("详情栏需要 DetailPanelProvider");
  return <aside ref={panel.element} id={panel.id} className="detail-panel" aria-label={label} hidden={!panel.open}
    onKeyDown={event => {
      if (event.key === "Escape" && !event.defaultPrevented && !(event.target as Element).closest('[role="dialog"]')) {
        event.stopPropagation(); panel.close();
      }
    }}>
    <FooterContext.Provider value={footer}>
      <div className="detail-panel-scroll">{children}</div>
      <div className="detail-panel-footer" ref={setFooter} />
    </FooterContext.Provider>
  </aside>;
}

/** 保持表单状态由业务组件拥有，只把输入区显示在详情栏底部。 */
export function DetailPanelFooter({ children }: { children: ReactNode }) {
  const target = useContext(FooterContext);
  return target ? createPortal(children, target) : <>{children}</>;
}

export function DetailLayout({ children, panel, mainClassName = "", label }: {
  children: ReactNode;
  panel: ReactNode;
  mainClassName?: string;
  label?: string;
}) {
  return <div className="detail-workspace">
    <div className={`detail-main ${mainClassName}`}>{children}</div>
    <DetailSidePanel label={label}>{panel}</DetailSidePanel>
  </div>;
}
