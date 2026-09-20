
/** 页头：面包屑返回 + 标题 + 副标题 + 右侧动作区。 */
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
      {back && onBack && (
        <button className="back-link" onClick={onBack}>← {back}</button>
      )}
      <div className="page-head">
        <div style={{ minWidth: 0 }}>
          <h1>{title}</h1>
          {sub && <p className="page-sub" style={{ marginBottom: 8 }}>{sub}</p>}
          {children}
        </div>
        {actions && <div className="actions">{actions}</div>}
      </div>
    </>
  );
}
