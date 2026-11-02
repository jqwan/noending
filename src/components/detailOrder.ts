/** 时间未知或无效的排在最后；同一时间保留原顺序，也不修改调用方的数组。 */
export function recentFirst<T>(items: readonly T[], date: (item: T) => string | null | undefined): T[] {
  const timestamp = (value: string | null | undefined) => {
    const parsed = value ? Date.parse(value) : NaN;
    return Number.isFinite(parsed) ? parsed : -Infinity;
  };
  return [...items].sort((a, b) => timestamp(date(b)) - timestamp(date(a)) || 0);
}
