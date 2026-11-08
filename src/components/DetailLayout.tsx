import { createContext, useContext, useId, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { booleanPreference, useViewPreference } from "../hooks/useViewState";
import PanelToggle from "./PanelToggle";
import PanelResizer from "./PanelResizer";

type Panel = {
  kind: string;
  group: string;
  open: boolean;
  width: number;
  resize: (width: number) => void;
  id: string;
  toggle: () => void;
  close: () => void;
  hint?: string;
  button: React.RefObject<HTMLButtonElement>;
  element: React.RefObject<HTMLElement>;
};
const PanelContext = createContext<Panel | null>(null);
const SectionKindContext = createContext<string | null>(null);
const FooterContext = createContext<HTMLElement | null>(null);
const validWidth = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 280 && value <= 480;

export function DetailPanelProvider({ kind, children, hint }: { kind: string; children: ReactNode; hint?: string }) {
  // 同组子页共用右栏及其卡片偏好，主区卡片仍使用各自页面类型。
  const group = kind === "collaboration" ? "task" : kind === "terminal" ? "session" : kind;
  const [open, setOpen] = useViewPreference(`details.${group}.panel`, !(window.matchMedia?.("(max-width: 900px)").matches ?? false), booleanPreference);
  const [width, setWidth] = useViewPreference(`details.${group}.panelWidth`, 360, validWidth);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const element = useRef<HTMLElement>(null);
  const close = () => {
    if (element.current?.contains(document.activeElement)) button.current?.focus();
    setOpen(false);
  };
  return <PanelContext.Provider value={{ kind, group, open, width, resize: setWidth, id, button, element, close, hint, toggle: () => open ? close() : setOpen(true) }}>{children}</PanelContext.Provider>;
}

export function useDetailSectionKind() {
  const sectionKind = useContext(SectionKindContext);
  const panel = useContext(PanelContext);
  return sectionKind ?? panel?.kind ?? "detail";
}

/** 提供给持续挂载的标题栏，操作仍指向当前页面的详情栏。 */
export function useDetailPanel() {
  return useContext(PanelContext);
}

/** PageHeader 自动提供统一入口，也可在专属工具栏中单独使用。 */
export function DetailPanelToggle({ panel: suppliedPanel }: { panel?: Panel | null } = {}) {
  const context = useContext(PanelContext);
  const panel = suppliedPanel === undefined ? context : suppliedPanel;
  if (!panel) return null;
  return <div className="detail-panel-control page-head-group"><PanelToggle ref={panel.button} side="right" open={panel.open} label="详情栏"
    controls={panel.id} hint={panel.hint} onToggle={panel.toggle} /></div>;
}

export function DetailSidePanel({ children, label = "详情栏" }: { children: ReactNode; label?: string }) {
  const panel = useContext(PanelContext);
  const [footer, setFooter] = useState<HTMLDivElement | null>(null);
  if (!panel) throw new Error("详情栏需要 DetailPanelProvider");
  return <aside ref={panel.element} id={panel.id} className="detail-panel" aria-label={label} hidden={!panel.open}
    style={{ "--detail-panel-width": `${panel.width}px` } as CSSProperties}
    onKeyDown={event => {
      if (event.key === "Escape" && !event.defaultPrevented && !(event.target as Element).closest('[role="dialog"]')) {
        event.stopPropagation(); panel.close();
      }
    }}>
    <PanelResizer side="right" label="详情栏宽度" controls={panel.id} value={panel.width}
      min={280} max={480} defaultValue={360} onResize={panel.resize} className="detail-panel-resizer" />
    <FooterContext.Provider value={footer}>
      <div className="detail-panel-scroll"><SectionKindContext.Provider value={panel.group}>{children}</SectionKindContext.Provider></div>
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
