/**
 * 用量数字的共享口径：统计面板与会话详情页的执行统计读同一份账本，
 * 展示也必须同一套——缩写格式、合并规则、缓存命中公式各只有这一份。
 */

/** 大数缩写；精确值放在 title 里。null = 该轴没人记录，不冒充 0。 */
export function fmtTokens(n: number | null): string {
  if (n === null) return "—";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 10_000) return `${(n / 1e3).toFixed(1)}k`;
  return n.toLocaleString("en-US");
}

/** 合并已记录的 Token 轴；全部缺失时保留未知状态。 */
export function sumTokens(...values: (number | null)[]): number | null {
  return values.every((value) => value === null)
    ? null
    : values.reduce<number>((total, value) => total + (value ?? 0), 0);
}

export const CACHE_HIT_HINT =
  "按用量账本 Token 加权：缓存读取 ÷ (非缓存输入 + 缓存读取)。没有输入用量时显示 —。";

/** 缓存命中率：与后端 usage_overview 同一分式；没有输入用量时是未知，不是 0。 */
export function cacheHitRate(
  input: number | null,
  cached: number | null,
): number | null {
  const denominator = (input ?? 0) + (cached ?? 0);
  if (denominator <= 0) return null;
  return (cached ?? 0) / denominator;
}

export function fmtCacheHit(rate: number | null): string {
  return rate === null ? "—" : `${(rate * 100).toFixed(2)}%`;
}
