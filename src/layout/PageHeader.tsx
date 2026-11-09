import { createContext, useCallback, useContext, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { DetailPanelToggle, useDetailPanel } from "../components/DetailLayout";

type HeaderProps = {
  back?: string;
  onBack?: () => void;
  title: string;
  icon?: ReactNode;
  sub?: ReactNode;
  navigation?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
};
type HeaderModel = { owner: object; props: HeaderProps; panel: ReturnType<typeof useDetailPanel> };
const HeaderHostContext = createContext<{
  publish: (model: HeaderModel) => void;
  remove: (owner: object) => void;
} | null>(null);

/** 子页只提交当前标题栏内容，导航按钮与焦点由共同容器保留。 */
export function PageHeaderHost({ children }: { children: ReactNode }) {
  const [model, setModel] = useState<HeaderModel | null>(null);
  const remove = useCallback((owner: object) => setModel(current => current?.owner === owner ? null : current), []);
  const host = useMemo(() => ({ publish: setModel, remove }), [remove]);
  return <HeaderHostContext.Provider value={host}>
    <div className="main fill detail-page session-page-shell">
      <HeaderBar {...(model?.props ?? { title: "" })} panel={model?.panel ?? null} />
      {children}
    </div>
  </HeaderHostContext.Provider>;
}

/**
 * 标题图标、导航切换、业务操作与右栏开关使用独立槽位，间距和分隔由公共样式统一管理。
 * 页头：标题行（标题 + 动作区）位于内容卡片顶部的 48px 页头行（`layout.css`
 * 的 `.page-head`），其后依次是返回链接、副标题、children。顺序不能倒，否则吸顶落不进去。
 *
 * `data-tauri-drag-region="deep"` 让页头空白处也能拖动窗口：取 `deep` 而非裸值——
 * 裸值只认"正好点在这个元素上"，标题与按钮之间的空隙都会落空。子元素里的 button 会被
 * Tauri 判为可点元素、自动不拖。
 *
 * 标题自身反过来标 `="false"` 把它从拖动区里摘出去：标题是内容不是窗口边框，
 * 按一下不该把整个窗口带走（也顺带去掉双击标题最大化）。tauri 的 drag.js 沿
 * composed path 上溯，遇到 `="false"` 直接返回——所以标题里的图标、文字一起豁免，
 * 而两者之间的空白仍是可拖的标题栏。
 */
export default function PageHeader(props: HeaderProps) {
  const { back, onBack, title, icon, sub, navigation, actions, children } = props;
  const panel = useDetailPanel();
  const host = useContext(HeaderHostContext);
  const owner = useRef({}).current;
  useLayoutEffect(() => {
    host?.publish({ owner, props: { title, icon, navigation, actions }, panel });
  }, [host, owner, title, icon, navigation, actions, panel]);
  useLayoutEffect(() => () => host?.remove(owner), [host, owner]);
  return <>
    {!host && <HeaderBar {...props} panel={panel} />}
    {back && onBack && <button className="back-link" onClick={onBack}>← {back}</button>}
    {sub && <p className="page-sub">{sub}</p>}
    {children}
  </>;
}

function HeaderBar({ title, icon, navigation, actions, panel }: HeaderProps & { panel: ReturnType<typeof useDetailPanel> }) {
  // 右栏打开时，操作区按右栏宽度切分；没有子页导航的详情页同样适用。
  const alignPanel = Boolean(panel?.open);
  return (
    <>
      <div className={`page-head${alignPanel ? " page-head-panel-open" : ""}`} data-tauri-drag-region="deep"
        style={alignPanel ? { "--detail-panel-width": `${panel!.width}px` } as CSSProperties : undefined}>
        <h1
          className="page-title"
          data-tauri-drag-region="false"
          title={title || undefined}
        >
          {icon && <span className="page-title-icon" aria-hidden="true">{icon}</span>}
          <span className="page-title-text">{title}</span>
        </h1>
        <div className="actions">
          {navigation && <div className="page-head-group page-head-navigation">{navigation}</div>}
          <div className="page-head-trailing">
            {actions && <div className="page-head-group">{actions}</div>}
            <DetailPanelToggle panel={panel} />
          </div>
        </div>
      </div>
    </>
  );
}
