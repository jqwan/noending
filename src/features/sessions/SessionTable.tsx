import { useViewState } from "../../hooks/useViewState";
import Icon from "../../components/Icon";
import { useMemo, useState } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type AgentStatusEntry, type Session } from "../../types";
import {
  continueSessionTerminalWithToast,
  desktopContinueState,
  resolveSessionContinueMode,
  useAgentStatus,
} from "./continueDesktop";
import type { Route } from "../../app/routes";

/* 展示层 helper —— SessionCards 与 SessionDetailView 共用。只做「怎么显示得下、
 * 看得懂」：截断与中文占位，不改领域字段；完整值永远可达，所以截断不损失
 * provenance（AGENTS.md: Context Provenance Fidelity）。 */

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
 * Project 单元格。v0.2 里 Project 只有一条事实链：`workspace_path_id →
 * WorkspacePath.project_id`；API 的 `session.project_id` 在读取时由该链派生。
 * 缺目录时分别说「待解析」/「无工作目录」。
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
      ? { text: name, hint: `工作目录派生 · ${name}`, dim: false }
      : {
        text: PROJECT_PENDING,
        hint: "项目信息加载中",
        dim: true,
      };
  }
  if (session.project_id) {
    const name = projectNameById.get(session.project_id);
    return {
      text: name ?? "未验证项目",
      hint: "未关联有效工作路径",
      dim: true,
    };
  }
  return (session.cwd ?? "").trim() === ""
    ? {
      text: PROJECT_NO_PATH,
      hint: "无工作目录记录",
      dim: true,
    }
    : {
      text: PROJECT_PENDING,
      hint: "未登记为工作路径",
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
 * 路径中段省略：末段目录名是识别工作目录的唯一凭据，必须保住。
 *
 * Windows 反斜杠与盘符（含 UNC `\\server\share\…`）按 Windows 规则切；
 * Unix 段名里合法的 `\` 不当分隔符（macOS 上 `my\dir` 是一个目录名）。
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

/* 列宽预算：td 由全局 CSS 限定 max-width 320px + nowrap（components.css
 * .session-table），1 个宽度单位 ≈ 7px。宁可 JS 先省，也不让浏览器做尾部
 * 省略（会吃掉末段），更不让 7 列撑出 .main 的 1200px。 */
const W_WORKSTREAM = 16;

/**
 * Sessions 卡片：信息按卡片分组，随窗口宽度自适应，点击进入详情。
 * 一行最多一个任务：`session.owner_workstream_id` 指向的那一个。
 */
export default function SessionCards({
  sessions,
  workstreamTitleById,
  projectNameById,
  viewMode = "cards",
  onOpen,
  onResume,
  onArchive,
  onRestore,
  onDelete,
  busy = false,
  onTerminalResume,
  navigate,
}: {
  sessions: Session[];
  /** Workstream id → 标题；用于给 `owner_workstream_id` 一个可读名字。 */
  workstreamTitleById: Map<string, string>;
  projectNameById: Map<string, string>;
  viewMode?: "cards" | "list";
  onOpen: (sessionId: string) => void;
  onResume: (sessionId: string) => void;
  onArchive: (sessionId: string) => void;
  onRestore?: (sessionId: string) => void;
  onDelete?: (sessionId: string) => void;
  busy?: boolean;
  onTerminalResume?: (session: Session) => void;
  navigate?: (r: Route) => void;
}) {
  const [visibleCount, setVisibleCount] = useViewState("sessions.visibleCount", 100);
  // 「继续」按钮的可用性需要桌面端在场事实：读一次 agent 状态。
  const agentStatus = useAgentStatus();
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

  if (viewMode === "list") {
    return (
      <div className="session-list" key="session-list">
        {rows.slice(0, visibleCount).map(({ session: s, workstream, workstreamFull, project }) => (
          <article className="session-list-row" key={`list-${s.id}`}>
            <button className="session-open" onClick={() => onOpen(s.id)}>
              <span className="session-list-title" title={sessionDisplayTitle(s.title)}>
                <AgentIcon agent={s.agent} size={15} />
                <span>{sessionDisplayTitle(s.title)}</span>
              </span>
              <span className="session-list-meta">
                <span>{agentDisplayLabel(s.agent)}</span>
                <span title={workstreamFull ?? undefined}>{workstream ?? "未归属任务"}</span>
                {!project.dim && <span title={project.hint}>{project.text}</span>}
                <span title={formatDateTime(s.last_activity_at ?? s.started_at)}>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
              </span>
              {s.cwd && (
                <span className="session-list-path" title={s.cwd}>
                  {cwdDisplayLabel(s.cwd, 90)}
                </span>
              )}
            </button>
            <div className="session-list-actions">
              <SessionResumeButton session={s} agentStatus={agentStatus} onResume={onResume} onTerminalResume={onTerminalResume} navigate={navigate} />
              {s.archived_at ? <>
                <button className="btn small ghost icon-button" disabled={busy} title="取消归档" aria-label={`取消归档${sessionDisplayTitle(s.title)}`} onClick={() => onRestore?.(s.id)}><Icon name="unarchive" /></button>
                <button className="btn small ghost icon-button danger" disabled={busy} title="永久删除" aria-label={`永久删除${sessionDisplayTitle(s.title)}`} onClick={() => onDelete?.(s.id)}><Icon name="trash" /></button>
              </> : <button className="btn small ghost icon-button" disabled={busy} title="归档" aria-label={`将${sessionDisplayTitle(s.title)}归档`} onClick={() => onArchive(s.id)}><Icon name="archive" /></button>}
            </div>
          </article>
        ))}
        {rows.length > visibleCount && (
          <div style={{ marginTop: 14, textAlign: "center" }}>
            <button className="btn small ghost" onClick={() => setVisibleCount((count) => count + 100)}>
              显示更多（剩余 {rows.length - visibleCount}）
            </button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="ws-grid board-grid" key="session-cards">
      {rows.slice(0, visibleCount).map(({ session: s, workstream, workstreamFull, project }) => (
        <article
          className="ws-card session-card full"
          key={`card-${s.id}`}
          tabIndex={0}
          role="button"
          onClick={() => onOpen(s.id)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onOpen(s.id);
            }
          }}
        >
          <header className="ws-card-head">
            <div className="session-card-title-wrap session-list-title" title={sessionDisplayTitle(s.title)}>
              <AgentIcon agent={s.agent} size={16} />
              <h3 className="ws-card-title" style={{ minWidth: 0, flex: 1 }}>
                <span className="card-title-link">{sessionDisplayTitle(s.title)}</span>
              </h3>
            </div>
            <span className="badge session-list-meta" title={agentDisplayLabel(s.agent)}>
              {agentDisplayLabel(s.agent)}
            </span>
          </header>

          <div className="session-card-body">
            <div className="session-card-prop" title={workstreamFull ?? undefined}>
              <Icon name="tasks" />
              <span className="truncate">
                {workstream ?? "未归属任务"}
              </span>
            </div>

            {!project.dim && (
              <div className="session-card-prop" title={project.hint}>
                <Icon name="folder" />
                <span className="truncate">{project.text}</span>
              </div>
            )}

            {s.cwd && (
              <div className="session-card-path" title={s.cwd}>
                <span className="mono">{cwdDisplayLabel(s.cwd, 40)}</span>
              </div>
            )}
          </div>

          <footer className="ws-card-meta session-card-meta">
            <span className="muted small" title={formatDateTime(s.last_activity_at ?? s.started_at)}>
              {timeAgo(s.last_activity_at ?? s.started_at)}
            </span>
            <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
              <SessionResumeButton session={s} agentStatus={agentStatus} onResume={onResume} onTerminalResume={onTerminalResume} navigate={navigate} />
              {s.archived_at ? <>
                <button className="btn small ghost icon-button" disabled={busy} title="取消归档" aria-label={`取消归档${sessionDisplayTitle(s.title)}`} onClick={() => onRestore?.(s.id)}><Icon name="unarchive" /></button>
                <button className="btn small ghost icon-button danger" disabled={busy} title="永久删除" aria-label={`永久删除${sessionDisplayTitle(s.title)}`} onClick={() => onDelete?.(s.id)}><Icon name="trash" /></button>
              </> : <button className="btn small ghost icon-button" disabled={busy} title="归档" aria-label={`将${sessionDisplayTitle(s.title)}归档`} onClick={() => onArchive(s.id)}><Icon name="archive" /></button>}
            </div>
          </footer>
        </article>
      ))}
      {rows.length > visibleCount && (
        <div style={{ gridColumn: "1 / -1", textAlign: "center", marginTop: 8 }}>
          <button className="btn small ghost" onClick={() => setVisibleCount((count) => count + 100)}>
            显示更多（剩余 {rows.length - visibleCount}）
          </button>
        </div>
      )}
    </div>
  );
}

/** 行内「继续」：图标按钮，与会话详情页右上角的继续会话按钮保持一致（桌面/终端），
 *  以聚合按钮的默认设置展示（非聚合分列）。
 *  - 兼具桌面与终端能力（codex）：按聚合按钮设置显示为桌面或终端图标；
 *  - 仅终端能力（claude_code / pi / antigravity_cli）：显示终端图标；
 *  - 仅桌面能力（antigravity_desktop / dsh / qoder / workbuddy / zcode）：显示桌面图标；
 *  - 点击时在对应桌面应用或内嵌终端中继续。 */
export function SessionResumeButton({
  session,
  agentStatus,
  onResume,
  onTerminalResume,
  navigate,
}: {
  session: Session;
  agentStatus: Record<string, AgentStatusEntry> | null;
  onResume: (sessionId: string) => void;
  onTerminalResume?: (session: Session) => void;
  navigate?: (r: Route) => void;
}) {
  const [terminalBusy, setTerminalBusy] = useState(false);
  const mode = resolveSessionContinueMode(session);

  if (mode === "terminal") {
    const handleTerminal = async (e: React.MouseEvent) => {
      e.stopPropagation();
      if (terminalBusy || session.archived_at) return;
      if (onTerminalResume) {
        onTerminalResume(session);
        return;
      }
      if (!navigate) return;
      setTerminalBusy(true);
      try {
        await continueSessionTerminalWithToast(session, navigate);
      } finally {
        setTerminalBusy(false);
      }
    };

    return (
      <button
        className="btn small ghost icon-button"
        title={session.archived_at ? "已归档的会话不能继续，请先取消归档" : "在终端中继续"}
        aria-label={`在终端中继续${sessionDisplayTitle(session.title)}`}
        disabled={Boolean(session.archived_at) || terminalBusy || (!onTerminalResume && !navigate)}
        onClick={handleTerminal}
      >
        <Icon name="terminal" />
      </button>
    );
  }

  const { disabled, title } = desktopContinueState(session, agentStatus?.[session.agent] ?? null);
  return (
    <button
      className="btn small ghost icon-button"
      title={title}
      aria-label={`在桌面应用中继续${sessionDisplayTitle(session.title)}`}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onResume(session.id);
      }}
    >
      <Icon name="desktop" />
    </button>
  );
}
