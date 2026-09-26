import { useViewState } from "../../hooks/useViewState";
import Icon from "../../components/Icon";
import { useMemo } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type Session } from "../../types";

/* ------------------------------------------------------------------ *
 * 展示层 helper —— SessionCards 与 SessionDetailView 共用。
 *
 * 这里只做「怎么显示得下、看得懂」：截断与中文占位。领域字段一个都不改，
 * 完整值永远可达（title 提示 → Detail 页 → 复制按钮），所以截断不会损失
 * provenance（AGENTS.md: Context Provenance Fidelity）。
 * ------------------------------------------------------------------ */

/** 无标题 / title 未知的 Session：不拿 root_agent_session_id 当标题糊用户。 */
export const UNTITLED_SESSION = "未命名会话";

/** 无 cwd 的 Session 仍然合法（Workstream 不是路径，Session 也不持有路径）。 */
export const NO_CWD = "未设置";

/** 后端新增未知 Agent 时的兜底，不留空白单元格。 */
export const UNKNOWN_AGENT = "未知 Agent";

/** Workstream 标题为空串时的兜底（Session 可以未归属，但不能显示成空白格）。 */
export const UNNAMED_WORKSTREAM = "未命名任务";

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

/**
 * Project 单元格。
 *
 * v0.2 里 Project 只有一条事实链：`workspace_path_id → WorkspacePath.project_id`。
 * `sessions.project_id` 仍是缓存列，所以表格读它，但**读到什么就要说什么**：
 *
 * - 有 WorkspacePath → 名字是派生结果；
 * - 只有缓存值、没有 WorkspacePath → 派生链无法验证这个缓存列，
 *   弱化显示并说明它已经不决定任何事（值不删，但也不再冒充事实）；
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
        hint: "工作路径已经登记，但对应的项目名称还没读到（正在加载，或项目刚刚变化）。",
        dim: true,
      };
  }
  if (session.project_id) {
    const name = projectNameById.get(session.project_id);
    return {
      text: name ?? "未验证的项目",
      hint: "会话行上缓存的 project_id；这条会话没有可解析的工作路径，项目已经不由它决定。",
      dim: true,
    };
  }
  return (session.cwd ?? "").trim() === ""
    ? {
      text: PROJECT_NO_PATH,
      hint: "原始记录里没有工作目录，所以没有可派生的项目。",
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
export function shrinkMiddle(text: string, max: number): string {
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
const W_WORKSTREAM = 16;

/**
 * Sessions 卡片（整体设计）：信息按卡片分组，随窗口宽度自适应。
 * 点击卡片进入详情，操作区提供继续（Resume）和移入回收站。
 *
 * 一行最多一个任务：`session.owner_workstream_id` 指向的那一个，
 * 标题由调用方给出的 id → title 投影解析；未归属时显示「未归属任务」。
 */
export default function SessionCards({ sessions, workstreamTitleById, projectNameById, onOpen, onResume, onTrash }: {
  sessions: Session[];
  /** Workstream id → 标题；用于给 `owner_workstream_id` 一个可读名字。 */
  workstreamTitleById: Map<string, string>;
  projectNameById: Map<string, string>;
  onOpen: (sessionId: string) => void;
  onResume: (sessionId: string) => void;
  onTrash: (sessionId: string) => void;
}) {
  const [visibleCount, setVisibleCount] = useViewState("sessions.visibleCount", 100);
  const rows = useMemo(
    () => sessions.map((s) => {
      const wsTitle = s.owner_workstream_id
        ? (workstreamTitleById.get(s.owner_workstream_id)?.trim() || UNNAMED_WORKSTREAM)
        : null;
      return {
        session: s,
        workstream: wsTitle ? ellipsisTail(wsTitle, W_WORKSTREAM) : null,
        workstreamFull: wsTitle,
        project: projectCellFor(s, projectNameById),
      };
    }),
    [sessions, workstreamTitleById, projectNameById],
  );

  return (
    <div className="session-list">
      {rows.slice(0, visibleCount).map(({ session: s, workstream, workstreamFull, project }) => (
        <article className="session-list-row" key={s.id}>
          <button className="session-open" onClick={() => onOpen(s.id)}>
            <span className="session-list-title" title={sessionDisplayTitle(s.title)}>{sessionDisplayTitle(s.title)}</span>
            <span className="session-list-meta">
              <span><AgentIcon agent={s.agent} />{agentDisplayLabel(s.agent)}</span>
              <span title={workstreamFull ?? undefined}>所属任务: {workstream ?? "未归属任务"}</span>
              {!project.dim && <span title={project.hint}>{project.text}</span>}
              <span title={formatDateTime(s.last_activity_at ?? s.started_at)}>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
            </span>
            {s.cwd && <span className="session-list-path" title={s.cwd}>{cwdDisplayLabel(s.cwd, 90)}</span>}
          </button>
          <div className="session-list-actions">
            <button className="btn small" onClick={() => onResume(s.id)}>继续</button>
            <button className="btn small ghost icon-button" title="移入回收站" aria-label={`将${sessionDisplayTitle(s.title)}移入回收站`} onClick={() => onTrash(s.id)}><Icon name="trash" /></button>
          </div>
        </article>
      ))}
      {rows.length > visibleCount && <button className="btn small ghost" onClick={() => setVisibleCount(count => count + 100)}>显示更多（剩余 {rows.length - visibleCount}）</button>}
    </div>
  );
}
