
/**
 * 页头：标题行（标题 + 动作区）被 CSS 吸进应用标题栏那条 48px band（`layout.css`
 * 的 `.page-head`），其后依次是返回链接、副标题、children。顺序不能倒，否则吸顶落不进去。
 *
 * `data-tauri-drag-region="deep"` 是拖动窗口的关键：这条 band 的 z-index 盖住了 AppShell
 * 里那条 `.window-drag-region`。取 `deep` 而非裸值——裸值只认"正好点在这个元素上"，
 * 标题文字与按钮间的空隙都会落空。子元素里的 button 会被 Tauri 判为可点元素、自动不拖。
 */
export default function PageHeader({ back, onBack, title, sub, actions, children }: {
  back?: string;
  onBack?: () => void;
  title: React.ReactNode;
  sub?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode; // meta 行等自定义内容
}) {
  return (
    <>
      <div className="page-head" data-tauri-drag-region="deep">
        <h1 className="page-title" title={typeof title === "string" ? title : undefined}>{title}</h1>
        {actions && <div className="actions">{actions}</div>}
      </div>
      {back && onBack && (
        <button className="back-link" onClick={onBack}>← {back}</button>
      )}
      {sub && <p className="page-sub">{sub}</p>}
      {children}
    </>
  );
}
