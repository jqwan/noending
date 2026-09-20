import { useMemo } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type Session, type SessionBindingRow } from "../../types";

/* ------------------------------------------------------------------ *
 * 展示层 helper —— SessionTable 与 SessionDetailView 共用。
 *
 * 这里只做「怎么显示得下、看得懂」：截断与中文占位。领域字段一个都不改，
 * 完整值永远可达（title 提示 → Detail 页 → 复制按钮），所以截断不会损失
 * provenance（AGENTS.md: Context Provenance Fidelity）。
 * ------------------------------------------------------------------ */

/** 无标题 / title 未知的 Session：不拿 agent_session_id 当标题糊用户。 */
export const UNTITLED_SESSION = "未命名 Session";

/** 无 cwd 的 Session 仍然合法（Workstream 不是路径，Session 也不绑定路径）。 */
export const NO_CWD = "未设置";

/** 后端新增未知 Agent 时的兜底，不留空白单元格。 */
export const UNKNOWN_AGENT = "未知 Agent";

/** Workstream 标题为空串时的兜底（Session 可以零关联，但不能显示成空白格）。 */
export const UNNAMED_WORKSTREAM = "未命名 Workstream";

/** 有 cwd、但还没被 NoEnding 登记成 WorkspacePath 的 Project 单元格状态。 */
export const PROJECT_PENDING = "待解析";

/** 连 cwd 都没有的 Project 单元格状态（v0.2：Project 只能从工作目录派生）。 */
export const PROJECT_NO_PATH = "无工作目录";

export function agentDisplayLabel(agent: Agent | null | undefined): string {
  const key = (agent ?? "") as string;
  return (AGENT_LABELS as Record<string, string>)[key] ?? UNKNOWN_AGENT;
}

/** title 由首条用户消息派生，可能为 null / 全空白。 */
export function sessionDisplayTitle(title: string | null | undefined): string {
  const t = (title ?? "").replace(/\s+/g, " ").trim();
  return t === "" ? UNTITLED_SESSION : t;
}

/** Binding role（§2.2）：主关联 / 相关关联。 */
export function bindingRoleLabel(role: string | null | undefined): string {
  if (role === "primary") return "主关联";
  if (role === "related") return "相关关联";
  return role ?? "相关关联";
}

/**
 * Project 单元格（方案 §1.10 / §43.4-2）。
 *
 * v0.2 里 Project 只有一条事实链：`workspace_path_id → WorkspacePath.project_id`。
 * `sessions.project_id` 仍是缓存列，所以表格读它，但**读到什么就要说什么**：
 *
 * - 有 WorkspacePath → 名字是派生结果；
 * - 只有缓存值、没有 WorkspacePath → 那是 v0.2 之前手工指派留下的历史标签，
 *   弱化显示并说明它已经不决定任何事（历史不删，但也不再冒充事实）；
 * - 有 cwd、还没有 WorkspacePath → 等目录扫描补上，说「待解析」；
 * - 什么都没有 → 空缺的原因是「没有记录过工作目录」，不是「没被归属」。
 */
export interface ProjectCell {
  text: string;
  hint: string;
  /** 弱化显示：值仍然在场，但它不是派生事实。 */
  dim: boolean;
}

export function projectCellFor(
  session: Pick<Session, "project_id" | "workspace_path_id" | "cwd">,
  projectNameById: Map<string, string>,
): ProjectCell {
  if (session.workspace_path_id) {
    const name = session.project_id ? projectNameById.get(session.project_id) ?? null : null;
    return name
      ? { text: name, hint: `由工作目录自动派生 · ${name}`, dim: false }
      : {
        text: PROJECT_PENDING,
        hint: "工作路径已经登记，但对应的 Project 名称还没读到（正在加载，或 Project 刚刚变化）。",
        dim: true,
      };
  }
  if (session.project_id) {
    const name = projectNameById.get(session.project_id);
    return {
      text: name ?? "历史 Project 标签",
      hint: "v0.2 之前手工指派留下的标签；这条 Session 没有可解析的工作路径，Project 已经不由它决定。",
      dim: true,
    };
  }
  return (session.cwd ?? "").trim() === ""
    ? {
      text: PROJECT_NO_PATH,
      hint: "原始记录里没有工作目录，所以没有可派生的 Project。",
      dim: true,
    }
    : {
      text: PROJECT_PENDING,
      hint: "工作目录还没有被登记成工作路径，NoEnding 会在下次目录扫描后自动补上。",
      dim: true,
    };
}

/** 东亚宽字符按 2 个宽度单位计，否则「80 个中文」会把表格撑破。 */
function widthOf(s: string): number {
  let w = 0;
  for (const ch of s) {
    const c = ch.codePointAt(0) ?? 0;
    const wide =
      (c >= 0x1100 && c <= 0x115f) ||
      c === 0x2329 || c === 0x232a ||
      (c >= 0x2e80 && c <= 0xa4cf && c !== 0x303f) ||
      (c >= 0xac00 && c <= 0xd7a3) ||
      (c >= 0xf900 && c <= 0xfaff) ||
      (c >= 0xfe30 && c <= 0xfe6f) ||
      (c >= 0xff00 && c <= 0xff60) ||
      (c >= 0xffe0 && c <= 0xffe6) ||
      (c >= 0x20000 && c <= 0x2fffd) ||
      (c >= 0x30000 && c <= 0x3fffd);
    w += wide ? 2 : 1;
  }
  return w;
}

/** 尾部省略：用在「靠前更重要」的文本上（标题、Workstream 名）。 */
export function ellipsisTail(text: string, max: number): string {
  if (widthOf(text) <= max) return text;
  const chars = [...text];
  let used = 0;
  let i = 0;
  for (; i < chars.length; i++) {
    const cw = widthOf(chars[i]);
    if (used + cw > max - 1) break;
    used += cw;
  }
  return chars.slice(0, i).join("").trimEnd() + "…";
}

/** 保头保尾、省中间：UUID / 超长目录名这种「两头都有信息」的字符串用。 */
function shrinkMiddle(text: string, max: number): string {
  if (text === "") return "";
  if (widthOf(text) <= max) return text;
  if (max <= 2) return "…";
  const chars = [...text];
  const budget = max - 1;                 // “…” 占 1 个宽度单位
  const headRoom = Math.ceil(budget / 2);
  const tailRoom = budget - headRoom;
  let head = 0;
  let used = 0;
  while (head < chars.length && used + widthOf(chars[head]) <= headRoom) {
    used += widthOf(chars[head]);
    head += 1;
  }
  let tail = 0;
  used = 0;
  while (tail < chars.length - head && used + widthOf(chars[chars.length - 1 - tail]) <= tailRoom) {
    used += widthOf(chars[chars.length - 1 - tail]);
    tail += 1;
  }
  return chars.slice(0, head).join("") + "…" + (tail > 0 ? chars.slice(chars.length - tail).join("") : "");
}

/**
 * 路径中段省略：末段目录名是用户识别工作目录的唯一凭据，必须保住，
 * 所以宁可省中间也不省尾巴。
 *
 * Windows 反斜杠与盘符（含 UNC `\\server\share\…`）按 Windows 规则切，
 * 不会当成 Unix 路径；Unix 段名里合法的 `\` 也不当分隔符（macOS 上
 * `my\dir` 是一个目录名）。原文放得下时原样返回，只在真溢出时才省。
 */
export function ellipsisPathMiddle(path: string, max: number): string {
  const trimmed = path.trim();
  if (trimmed === "" || widthOf(trimmed) <= max) return trimmed;

  const rootMatch = /^([A-Za-z]:[\\/]|\\\\[^\\/]*[\\/]|\/)/.exec(trimmed);
  const root = rootMatch ? rootMatch[1] : "";
  // 盘符与 UNC 才两种分隔符通吃（Windows 本来就混用 / 和 \）；POSIX 路径里
  // `\` 是合法文件名字符，跟着当分隔符切会把 `my\dir` 拆成两段，编造出层级。
  const isWindowsPath = /^[A-Za-z]:[\\/]|\\\\/.test(trimmed);
  const segs = trimmed
    .slice(root.length)
    .split(isWindowsPath ? /[\\/]+/ : "/")
    .filter((s) => s.length > 0);
  if (segs.length === 0) return ellipsisTail(trimmed, max);

  const sep = isWindowsPath ? "\\" : "/";
  const tail = segs[segs.length - 1];
  const head = segs.slice(0, -1);
  // Unix 根 "/" 不携带身份信息，省掉它换宽度；盘符与 UNC 主机名要留。
  const keepRoot = root === "/" ? "" : root;

  // 只显示 head 的「连续后缀」：一旦某段太长放不下就停在那里，宁可少显示父目录，
  // 也不做跳跃式取段 —— 那样 `…` 就不再等于「这里省掉了相邻几段」，会误导识别。
  for (let k = head.length - 1; k >= 0; k--) {
    const parts = [...head.slice(head.length - k), tail];
    const candidate = `${keepRoot}…${sep}${parts.join(sep)}`;
    if (widthOf(candidate) <= max) return candidate;
  }

  // 连根前缀都放不下时，宁可丢根（最不携带身份的部分）也不要把盘符/主机名截断。
  const bare = `…${sep}${tail}`;
  if (keepRoot && widthOf(bare) <= max) return bare;

  // 末段本身太长：压末段，仍保头保尾，绝不做纯尾部省略。
  const prefix = `${keepRoot}…${sep}`;
  const room = max - widthOf(prefix);
  if (room >= 3) return prefix + shrinkMiddle(tail, room);
  return shrinkMiddle(trimmed, max);
}

export function cwdDisplayLabel(cwd: string | null | undefined, max: number): string {
  const trimmed = (cwd ?? "").trim();
  if (trimmed === "") return NO_CWD;
  return ellipsisPathMiddle(trimmed, max);
}

/** ISO → 本地可读时间；null / 非法值给出可读占位而不是 Invalid Date。 */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getFullYear()}/${pad(t.getMonth() + 1)}/${pad(t.getDate())} `
    + `${pad(t.getHours())}:${pad(t.getMinutes())}`;
}

/** 相对时间 + 绝对时间：列表要看快慢，Detail 要看具体时刻。 */
export function activityLabel(iso: string | null | undefined): string {
  const relative = timeAgo(iso);
  if (!iso) return relative;
  return `${relative} · ${formatDateTime(iso)}`;
}

/* ------------------------------- 列宽预算 ------------------------------- *
 * td 由全局 CSS 限定 max-width 320px + nowrap（components.css .session-table）。
 * 1 个宽度单位 ≈ 7px，所以这些预算就是列的真实宽度来源：宁可 JS 先省，也不
 * 让浏览器做尾部省略（那会把末段吃掉），更不让 7 列撑出 .main 的 1200px。 */
const W_TITLE = 34;
const W_WORKSTREAM = 16;
const W_PROJECT = 12;
const W_CWD = 26;

/**
 * 主关联优先（§2.2 词表）：多绑定 Session 不能显示成随机的那一个。
 * `roleOf` 让 SessionBindingRow 与 [binding, title] 元组两种形状共用同一个判断。
 */
export function primaryFirst<T>(rows: T[], roleOf: (row: T) => string): T[] {
  return [...rows].sort((a, b) => (roleOf(a) === "primary" ? 0 : 1) - (roleOf(b) === "primary" ? 0 : 1));
}

/**
 * Sessions 表格（整体设计方案 §38/§40）：Session 数量多，Table 优于 Card。
 * 主要操作只有两个：打开（Detail）与继续（Resume）。
 */
export default function SessionTable({ sessions, bindings, projectNameById, onOpen, onResume }: {
  sessions: Session[];
  bindings: Map<string, SessionBindingRow[]>;
  projectNameById: Map<string, string>;
  onOpen: (sessionId: string) => void;
  onResume: (sessionId: string) => void;
}) {
  const rows = useMemo(
    () => sessions.map((s) => {
      const bound = primaryFirst(bindings.get(s.id) ?? [], (r) => r.role);
      const wsTitle = bound[0] ? (bound[0].workstream_title.trim() || UNNAMED_WORKSTREAM) : null;
      return {
        session: s,
        workstream: wsTitle ? ellipsisTail(wsTitle, W_WORKSTREAM) : null,
        extraWorkstreams: Math.max(0, bound.length - 1),
        workstreamFull: wsTitle,
        project: projectCellFor(s, projectNameById),
      };
    }),
    [sessions, bindings, projectNameById],
  );

  return (
    <table className="session-table">
      <thead>
        <tr>
          <th style={{ width: 96 }}>Agent</th>
          <th style={{ width: 240 }}>Session</th>
          <th style={{ width: 150 }}>Workstream</th>
          <th style={{ width: 96 }} title="Project 由 Session 的工作目录自动派生，不能手工指派；这里只是一个分组视图">
            Project
          </th>
          <th style={{ width: 200 }}>工作目录</th>
          <th style={{ width: 96 }}>最近活动</th>
          <th style={{ width: 108 }}>操作</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(({ session: s, workstream, extraWorkstreams, workstreamFull, project }) => {
          const title = sessionDisplayTitle(s.title);
          const untitled = title === UNTITLED_SESSION;
          const cwd = (s.cwd ?? "").trim();
          return (
            <tr
              key={s.id}
              onClick={() => onOpen(s.id)}
              title={`打开 Session：${title}`}
            >
              <td className="cell-agent">
                <AgentIcon agent={s.agent} />
                {agentDisplayLabel(s.agent)}
              </td>
              <td className="cell-title" title={untitled ? `${UNTITLED_SESSION} · ${s.agent_session_id}` : title}>
                {ellipsisTail(title, W_TITLE)}
              </td>
              <td
                className={workstream ? undefined : "muted"}
                title={workstream ? `${workstreamFull}${extraWorkstreams > 0 ? " 等多条关联" : ""}` : "未关联 Workstream"}
              >
                {workstream ?? "未关联"}
                {extraWorkstreams > 0 && (
                  <span className="badge" style={{ marginLeft: 6 }}>+{extraWorkstreams}</span>
                )}
              </td>
              <td className={project.dim ? "muted" : undefined} title={project.hint}>
                {ellipsisTail(project.text, W_PROJECT)}
              </td>
              <td className={cwd ? "mono" : "muted"} title={cwd || "未设置工作目录"}>
                {cwdDisplayLabel(cwd, W_CWD)}
              </td>
              <td title={activityLabel(s.last_activity_at ?? s.started_at)}>
                {timeAgo(s.last_activity_at ?? s.started_at)}
              </td>
              <td onClick={(e) => e.stopPropagation()}>
                <div className="row" style={{ gap: 6 }}>
                  <button className="btn small ghost" onClick={() => onOpen(s.id)}>打开</button>
                  <button className="btn small" onClick={() => onResume(s.id)}>继续</button>
                </div>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
