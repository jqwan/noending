import type { Agent, PackageEntry, PackageOrigin, PackageToggleState } from "../../types";

export function packageDetailRoute(entry: PackageEntry) {
  return { view: "package" as const, packageId: entry.id, initialTitle: entry.name, agent: entry.agent ?? undefined };
}

export const ORIGIN_LABELS: Record<PackageOrigin, string> = {
  market: "市场安装",
  bundled: "Agent 内置",
  noending_managed: "NoEnding 内置",
  local: "本地安装",
  profile: "配置档安装",
};

/** Board cards show the origin class only; the marketplace name lives in the
 *  detail page and tooltips. */
export function originClassLabel(entry: PackageEntry): string {
  return ORIGIN_LABELS[entry.origin];
}

/** Display version: strips build metadata (`+…`, `-wb.…`) that never fits a
 *  card; the full string stays in the tooltip and the detail page. */
export function shortVersion(version: string): string {
  let display = version.split("+")[0];
  if (display.includes("-wb.")) display = display.split("-wb.")[0];
  return display.length > 24 ? `${display.slice(0, 24)}…` : display;
}

export function originLabel(entry: PackageEntry): string {
  if (entry.marketplace && entry.origin === "market") return `市场 · ${entry.marketplace}`;
  if (entry.marketplace && entry.origin === "profile") return `配置档 · ${entry.marketplace}`;
  return ORIGIN_LABELS[entry.origin];
}

/** Board badge: what the owning Agent reports right now, never merged across
 *  scopes. Unset (never written) counts as 未启用; unknown = no evidence. */
export function entryState(entry: PackageEntry): PackageToggleState {
  return entry.scopes[0]?.state ?? "unknown";
}

export function agentLabel(agent: Agent | null): string {
  if (!agent) return "未安装到任何 Agent";
  return agent;
}


