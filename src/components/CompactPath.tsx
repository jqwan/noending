/** 列表路径随容器宽度省略，优先保留末级名称；原始路径仍用于可访问名称。 */
export default function CompactPath({ path }: { path: string }) {
  const leaf = path.match(/[^\\/]+[\\/]*$/);
  const split = Math.max(0, (leaf?.index ?? 0) - 1);
  return <span className="compact-path">
    <span className="sr-only">{path}</span>
    <span className="compact-path-visible" aria-hidden="true">
      {split > 0 && <span className="compact-path-prefix">{path.slice(0, split)}</span>}
      <span className="compact-path-leaf"><bdi dir="ltr">{path.slice(split)}</bdi></span>
    </span>
  </span>;
}
