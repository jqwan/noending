import { createContext, useContext, useId, useRef, type ReactNode } from "react";
import { booleanPreference, useViewPreference } from "../hooks/useViewState";
import { useDetailSectionKind } from "./DetailLayout";
import Icon from "./Icon";
import { useLoadingDelay } from "../hooks/useLoadingDelay";

/** 快速读取不插入短暂提示；空内容只在读取完成后由业务卡片展示。 */
export function DetailLoading({ children }: { children: ReactNode }) {
  const visible = useLoadingDelay(true);
  return visible ? <p className="detail-empty" role="status">{children}</p> : null;
}

type SectionControls = { expanded: boolean; expand: () => void };
const SectionContext = createContext<SectionControls | null>(null);

export function useDetailSectionControls() { return useContext(SectionContext); }

/** 同类详情共用折叠偏好；正文保持挂载，保留草稿和运行中的操作。 */
export function DetailSection({ sectionId, label, title, actions, status, children, className = "", defaultExpanded = true }: {
  sectionId: string;
  label: string;
  title: ReactNode;
  actions?: ReactNode | ((controls: SectionControls) => ReactNode);
  status?: ReactNode;
  children: ReactNode;
  className?: string;
  defaultExpanded?: boolean;
}) {
  const kind = useDetailSectionKind();
  const [expanded, setExpanded] = useViewPreference(`details.${kind}.sections.${sectionId}`, defaultExpanded, booleanPreference);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const controls = { expanded, expand: () => setExpanded(true) };
  const toggle = () => {
    if (expanded && body.current?.contains(document.activeElement)) button.current?.focus();
    setExpanded(value => !value);
  };
  return <SectionContext.Provider value={controls}><section className={`card detail-section ${className}`} aria-label={label}>
    <DetailSectionHeader title={<button ref={button} type="button" className="detail-section-toggle"
      aria-expanded={expanded} aria-controls={id} title={`${expanded ? "收起" : "展开"}${label}`}
      onClick={toggle}>
      {title}<Icon name={expanded ? "chevronDown" : "chevronRight"} />
    </button>} actions={typeof actions === "function" ? actions(controls) : actions} />
    {status && <div className="detail-section-status">{status}</div>}
    <div ref={body} id={id} className="detail-section-body" hidden={!expanded}>{children}</div>
  </section></SectionContext.Provider>;
}

/** 详情卡片标题与操作共用一行；窄卡片允许操作自然换行。 */
function DetailSectionHeader({ title, actions }: {
  title: ReactNode;
  actions?: ReactNode;
}) {
  return <header className="detail-section-header">
    <h2 className="detail-section-title">{title}</h2>
    {actions && <div className="detail-section-actions">{actions}</div>}
  </header>;
}

export function DetailList({ children, label, compact = false }: { children: ReactNode; label: string; compact?: boolean }) {
  return <div className={`detail-list detail-scroll${compact ? " detail-list-compact" : ""}`} role="region" aria-label={label} tabIndex={0}>{children}</div>;
}

/** 长正文与事项只滚动内容，标题及操作始终留在卡片外层。 */
export function DetailContent({ children, label, className = "" }: { children: ReactNode; label: string; className?: string }) {
  return <div className={`detail-content detail-scroll ${className}`} role="region" aria-label={label} tabIndex={0}>{children}</div>;
}

/** 主入口与行尾操作是相邻控件，避免整行点击与嵌套按钮互相干扰。 */
export function DetailListRow({ icon, title, description, meta, actions, onOpen, titleHint }: {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  onOpen?: () => void;
  titleHint?: string;
}) {
  const hint = titleHint ?? (typeof title === "string" ? title : undefined);
  const content = <>
    {icon && <span className="detail-list-icon">{icon}</span>}
    <span className="detail-list-copy">
      <span className="detail-list-title">{title}</span>
      {description && <span className="detail-list-description">{description}</span>}
    </span>
  </>;
  return <div className="detail-list-row">
    {onOpen
      ? <button type="button" className="detail-list-open" title={hint} onClick={onOpen}>{content}</button>
      : <div className="detail-list-content" title={hint}>{content}</div>}
    {(meta || actions) && <div className="detail-list-trailing">
      {meta && <div className="detail-list-meta">{meta}</div>}
      {actions && <div className="detail-list-actions">{actions}</div>}
    </div>}
  </div>;
}

/** 与 common.Field 搭配使用，字段自身负责标签、值及复制操作。 */
export function DetailFields({ children }: { children: ReactNode }) {
  return <dl className="detail-fields detail-content detail-scroll" aria-label="属性字段" tabIndex={0}>{children}</dl>;
}
