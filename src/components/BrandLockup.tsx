import lockupUrl from "../assets/noending-lockup.svg";

// noending-lockup.svg 的画布是 831.61 × 124，其中字标 cap 高固定为 100 单位。
const CAP_UNITS = 100;
const VIEW_W = 831.61;
const VIEW_H = 124;

/**
 * 品牌横排锁定：标志 + 字标。cap 指字标大写字母高度，四周留白按画布比例自动带上。
 */
export default function BrandLockup({ cap = 20 }: { cap?: number }) {
  const scale = cap / CAP_UNITS;
  return (
    <span
      role="img"
      aria-label="NoEnding"
      className="brand-lockup"
      style={{
        width: VIEW_W * scale,
        height: VIEW_H * scale,
        maskImage: `url(${lockupUrl})`,
        WebkitMaskImage: `url(${lockupUrl})`,
      }}
    />
  );
}
