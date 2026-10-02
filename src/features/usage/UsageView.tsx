import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { useRefreshSignal } from "../../components/common";
import { AGENT_LABELS } from "../../types";
import { CACHE_HIT_HINT, fmtCacheHit, fmtTokens, sumTokens } from "./usageFormat";
import type { Route } from "../../app/routes";
import type {
  Agent,
  UsageAgentSlice,
  UsageCounts,
  UsageDaySlice,
  UsageModelSlice,
  UsageNamedSlice,
  UsageOverview,
  UsageRelationSlice,
  UsageSessionSlice,
} from "../../types";

/**
 * All token totals and model requests come from usage events;
 * message, tool and collaboration counts come from member activity stats.
 */

const DAY_BARS = 30;

// 数字与命中率的口径在 ./usageFormat（统计面板与详情页执行统计共用）。

function CacheHitCell({ rate }: { rate: number | null }) {
  return <td className="num" title={CACHE_HIT_HINT}>{fmtCacheHit(rate)}</td>;
}

function agentLabel(agent: string): string {
  return AGENT_LABELS[agent as Agent] ?? agent;
}

const RELATION_LABELS: Record<string, string> = {
  root: "根会话",
  child: "子会话",
  side: "辅会话",
};

/** 统计条的一格：和 token 大数卡同一套卡片 UI，数值是计数（全量显示）。 */
function StatsCell({ label, value }: { label: string; value: number }) {
  return (
    <div className="usage-total usage-stat">
      <div className="num" title={value.toLocaleString("en-US")}>
        {value.toLocaleString("en-US")}
      </div>
      <div className="lbl">{label}</div>
    </div>
  );
}

function TotalCell({ label, value }: { label: string; value: number | null }) {
  return (
    <div className="usage-total">
      <div className="num" title={value === null ? undefined : value.toLocaleString("en-US")}>
        {fmtTokens(value)}
      </div>
      <div className="lbl">{label}</div>
    </div>
  );
}

/** 最近 30 天的柱状序列：一根柱 = 一天，高度按 输入+输出。 */
function SeriesChart({ rows }: { rows: UsageDaySlice[] }) {
  const days = rows.slice(0, DAY_BARS).slice().reverse();
  if (days.length === 0) return null;
  const axis = (d: UsageDaySlice) => sumTokens(d.input_tokens, d.cached_tokens, d.output_tokens) ?? 0;
  const max = Math.max(...days.map(axis), 1);
  return (
    <div className="usage-series" role="img" aria-label={`最近 ${days.length} 天的每日用量`}>
      {days.map((d) => {
        const total = axis(d);
        return (
          <div key={d.day} className="usage-series-bar" title={`${d.day}：输入 ${fmtTokens(sumTokens(d.input_tokens, d.cached_tokens))} · 输出 ${fmtTokens(d.output_tokens)}`}>
            <div className="usage-series-fill" style={{ height: `${Math.max((total / max) * 100, total > 0 ? 4 : 1)}%` }} />
          </div>
        );
      })}
    </div>
  );
}

function LedgerHint() {
  return (
    <p className="usage-note">
      账本还没有数据：每模型 token、时间序列和调用类别要等来源重新摄入——在{" "}
      <span className="mono">设置 → 数据与高级</span> 对来源「从头重扫」，或删库重建一次即可补全全部历史。
    </p>
  );
}

const COUNT_COLUMNS: { key: keyof UsageCounts; label: string }[] = [
  { key: "root_members", label: "会话" },
  { key: "members", label: "会话成员" },
  { key: "user_messages", label: "用户消息" },
  { key: "agent_replies", label: "代理回复" },
  { key: "tool_calls", label: "工具调用" },
  { key: "side_activities", label: "代理协同" },
  { key: "requests", label: "模型请求" },
];

const RELATION_COUNT_COLUMNS = COUNT_COLUMNS.filter(({ key }) => key !== "root_members");

const ACTIVITY_COUNT_COLUMNS = COUNT_COLUMNS.filter(({ key }) => key !== "members" && key !== "root_members");

// ---------------- 可排序表 ----------------

type SortDir = "asc" | "desc";

/**
 * 一列 = 表头标签 + 排序键 + 单元格渲染。列定义驱动整张表，
 * 表头的点击排序与正文顺序由同一份列序保证，不会各说各话。
 */
interface Column<T> {
  key: string;
  label: string;
  /** 文本列第一次点击升序；数值列（默认）第一次点击降序——大的在前才有用。 */
  text?: boolean;
  /** 排序键；null 恒排最后（两个方向都是），与展示的「—」同一语义。 */
  value: (row: T) => string | number | null;
  cell: (row: T) => ReactNode;
}

function sortRows<T>(rows: T[], columns: Column<T>[], sort: { key: string; dir: SortDir } | null): T[] {
  if (!sort) return rows;
  const col = columns.find((c) => c.key === sort.key);
  if (!col) return rows;
  const keyed = rows.map((row) => ({ row, v: col.value(row) }));
  keyed.sort((a, b) => {
    if (a.v === null || b.v === null) {
      if (a.v === b.v) return 0;
      return a.v === null ? 1 : -1;
    }
    const cmp =
      typeof a.v === "string" || typeof b.v === "string"
        ? String(a.v).localeCompare(String(b.v), "zh-Hans-CN")
        : (a.v as number) - (b.v as number);
    return sort.dir === "asc" ? cmp : -cmp;
  });
  return keyed.map((k) => k.row);
}

const PAGE_SIZE = 10;

function SortableTable<T>({ columns, rows, rowKey }: {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
}) {
  // 默认序就是总量降序（后端同款 ORDER BY）——初始就带着排序态，
  // 让“总量”列头从加载起就亮着箭头，而不是等用户点过才有提示。
  const [sort, setSort] = useState<{ key: string; dir: SortDir }>({ key: "total", dir: "desc" });
  const [showAll, setShowAll] = useState(false);
  const toggle = (col: Column<T>) =>
    setSort((cur) =>
      cur?.key !== col.key
        ? { key: col.key, dir: col.text ? "asc" : "desc" }
        : { key: col.key, dir: cur.dir === "asc" ? "desc" : "asc" },
    );
  // 排序永远作用于全部行；前 10 只是显示裁剪——切换排序列时，
  // 新列的最大值可以从没展示过的行里冒上来。
  const sorted = useMemo(() => sortRows(rows, columns, sort), [rows, columns, sort]);
  const visible = showAll || sorted.length <= PAGE_SIZE ? sorted : sorted.slice(0, PAGE_SIZE);
  return (
    <div className="usage-table-scroll">
      <table className="usage-table">
        <thead>
          <tr>
            {columns.map((c) => {
              const dir = sort?.key === c.key ? sort.dir : null;
              return (
                <th key={c.key} className={c.text ? undefined : "num"}
                    aria-sort={dir ? (dir === "asc" ? "ascending" : "descending") : undefined}>
                  <button type="button" className="th-sort" title="点击按此列排序" onClick={() => toggle(c)}>
                    {c.label}
                    {dir && <span className="sort-arrow" aria-hidden="true" />}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {visible.map((row) => (
            <tr key={rowKey(row)}>
              {columns.map((c) => <Fragment key={c.key}>{c.cell(row)}</Fragment>)}
            </tr>
          ))}
        </tbody>
      </table>
      {sorted.length > PAGE_SIZE && (
        <button type="button" className="link" style={{ marginTop: 8 }}
          onClick={() => setShowAll((v) => !v)}>
          {showAll ? `收起，仅显示前 ${PAGE_SIZE}` : "显示全部"}
        </button>
      )}
    </div>
  );
}

// ---------------- 列定义 ----------------

function countCell(value: number | null): ReactNode {
  return (
    <td className="num" title={value === null ? "现有记录无法按模型归因" : undefined}>
      {value === null ? "—" : value.toLocaleString("en-US")}
    </td>
  );
}

function countColumn<T extends UsageCounts>(col: { key: keyof UsageCounts; label: string }): Column<T> {
  return {
    key: col.key,
    label: col.label,
    value: (r) => r[col.key],
    cell: (r) => countCell(r[col.key]),
  };
}

function tokenColumns<T extends {
  input_tokens: number | null;
  output_tokens: number | null;
  cached_tokens: number | null;
  cache_hit_rate: number | null;
}>(): Column<T>[] {
  return [
    { key: "input", label: "输入",
      value: (r) => sumTokens(r.input_tokens, r.cached_tokens),
      cell: (r) => <td className="num">{fmtTokens(sumTokens(r.input_tokens, r.cached_tokens))}</td> },
    { key: "output", label: "输出",
      value: (r) => r.output_tokens,
      cell: (r) => <td className="num">{fmtTokens(r.output_tokens)}</td> },
    { key: "hit", label: "缓存命中",
      value: (r) => r.cache_hit_rate,
      cell: (r) => <CacheHitCell rate={r.cache_hit_rate} /> },
    // 总量殿后：它是输入+输出之和，放最后与顶部大卡（总量·输入·输出·命中）呼应。
    { key: "total", label: "总量",
      value: (r) => sumTokens(r.input_tokens, r.cached_tokens, r.output_tokens),
      cell: (r) => <td className="num">{fmtTokens(sumTokens(r.input_tokens, r.cached_tokens, r.output_tokens))}</td> },
  ];
}

const AGENT_COLUMNS: Column<UsageAgentSlice>[] = [
  { key: "agent", label: "会话格式", text: true,
    value: (r) => agentLabel(r.agent),
    cell: (r) => <td>{agentLabel(r.agent)}</td> },
  ...COUNT_COLUMNS.map((c) => countColumn<UsageAgentSlice>(c)),
  ...tokenColumns<UsageAgentSlice>(),
];

// 「会话」= root_members（根成员数即会话数），「会话成员」= members——与统计格同序。
const MODEL_COLUMNS: Column<UsageModelSlice>[] = [
  { key: "model", label: "模型", text: true,
    value: (r) => r.display,
    cell: (r) => (
      <td className="mono" title={r.display === r.model ? undefined : r.model}>{r.display}</td>
    ) },
  { key: "root_members", label: "会话",
    value: (r) => r.root_members,
    cell: (r) => <td className="num">{r.root_members.toLocaleString("en-US")}</td> },
  { key: "members", label: "会话成员",
    value: (r) => r.members,
    cell: (r) => <td className="num">{r.members.toLocaleString("en-US")}</td> },
  { key: "requests", label: "模型请求",
    value: (r) => r.requests,
    cell: (r) => <td className="num">{r.requests.toLocaleString("en-US")}</td> },
  ...tokenColumns<UsageModelSlice>(),
];

const RELATION_COLUMNS: Column<UsageRelationSlice>[] = [
  { key: "relation", label: "成员类别", text: true,
    value: (r) => RELATION_LABELS[r.relation] ?? r.relation,
    cell: (r) => <td>{RELATION_LABELS[r.relation] ?? r.relation}</td> },
  ...RELATION_COUNT_COLUMNS.map((c) => countColumn<UsageRelationSlice>(c)),
  ...tokenColumns<UsageRelationSlice>(),
];

function AgentTable({ rows }: { rows: UsageAgentSlice[] }) {
  return <SortableTable columns={AGENT_COLUMNS} rows={rows} rowKey={(r) => r.agent} />;
}

function ModelTable({ rows }: { rows: UsageModelSlice[] }) {
  return <SortableTable columns={MODEL_COLUMNS} rows={rows} rowKey={(r) => r.model} />;
}

function NamedTable({ rows, label, onOpen }: { rows: UsageNamedSlice[]; label: string; onOpen?: (id: string) => void }) {
  const columns = useMemo<Column<UsageNamedSlice>[]>(() => [
    { key: "name", label, text: true,
      value: (r) => r.name,
      cell: (r) => (
        <td className="usage-session-title" title={r.name}>
          <span className="cell-clip">
            {onOpen ? (
              <button type="button" className="link usage-session-link" onClick={() => onOpen(r.id)}>{r.name}</button>
            ) : r.name}
          </span>
        </td>
      ) },
    ...COUNT_COLUMNS.map((c) => countColumn<UsageNamedSlice>(c)),
    ...tokenColumns<UsageNamedSlice>(),
  ], [label, onOpen]);
  return <SortableTable columns={columns} rows={rows} rowKey={(r) => r.id} />;
}

function TopSessions({ rows, navigate }: {
  rows: UsageSessionSlice[];
  navigate: (r: Route) => void;
}) {
  const columns = useMemo<Column<UsageSessionSlice>[]>(() => [
    { key: "title", label: "会话", text: true,
      value: (r) => r.title ?? "",
      cell: (r) => (
        <td className="usage-session-title" title={r.title ?? undefined}>
          <span className="cell-clip">
            <button
              type="button"
              className="link usage-session-link"
              onClick={() => navigate({ view: "session", sessionId: r.session_id })}
            >
              {r.title ?? "（无标题）"}
            </button>
          </span>
        </td>
      ) },
    // 该会话自己的成员图规模（根 + 子 + 辅）。
    { key: "members", label: "会话成员",
      value: (r) => r.members,
      cell: (r) => <td className="num">{r.members.toLocaleString("en-US")}</td> },
    ...ACTIVITY_COUNT_COLUMNS.map((c) => countColumn<UsageSessionSlice>(c)),
    ...tokenColumns<UsageSessionSlice>(),
  ], [navigate]);
  return <SortableTable columns={columns} rows={rows} rowKey={(r) => r.session_id} />;
}

export default function UsageView({ navigate }: { navigate: (r: Route) => void }) {
  const [data, setData] = useState<UsageOverview | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(() => {
    api
      .getUsageOverview()
      .then((o) => {
        setData(o);
        setError("");
      })
      .catch((e) => setError(`读取用量失败：${String(e)}`));
  }, []);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const ledgerEmpty = data !== null && data.ledger_events === 0;

  return (
    <div className="main usage-page">
      <PageHeader
        title="统计"
        sub="全库 Token 用量与模型归因"
      />

      {error !== "" && (
        <div className="card hairline" style={{ padding: 14 }}>
          <div className="small" style={{ color: "var(--danger)" }}>{error}</div>
          <button className="btn small" style={{ marginTop: 8 }} onClick={refresh}>重试</button>
        </div>
      )}

      {error === "" && data === null && <div className="muted">加载中…</div>}

      {data !== null && data.sessions === 0 && (
        <EmptyState
          title="还没有会话数据"
          hint="摄入到会话后，这里会汇总全部 Token 用量与模型分布。"
        />
      )}

      {data !== null && data.sessions > 0 && (
        <>
          <div className="usage-totals">
            <TotalCell label="总量" value={sumTokens(data.input_tokens, data.cached_tokens, data.output_tokens)} />
            <TotalCell label="输入" value={sumTokens(data.input_tokens, data.cached_tokens)} />
            <TotalCell label="输出" value={data.output_tokens} />
            <div className="usage-total">
              <div className="num" title={CACHE_HIT_HINT}>{fmtCacheHit(data.cache_hit_rate)}</div>
              <div className="lbl">缓存命中</div>
            </div>
          </div>
          <div className="usage-stats">
            <StatsCell label="会话" value={data.root_members} />
            <StatsCell label="会话成员" value={data.members} />
            <StatsCell label="用户消息" value={data.user_messages} />
            <StatsCell label="代理回复" value={data.agent_replies} />
            <StatsCell label="工具调用" value={data.tool_calls} />
            <StatsCell label="代理协同" value={data.side_activities} />
            <StatsCell label="模型请求" value={data.requests} />
          </div>
          {data.series.length > 0 && (
            <section className="card hairline usage-card">
              <h2>最近 {Math.min(data.series.length, DAY_BARS)} 天</h2>
              <SeriesChart rows={data.series} />
            </section>
          )}

          <p className="usage-note usage-dimension-note">
            下表的会话成员包含根、子、辅成员；输入包含缓存读取，缓存命中按账本 Token 汇总计算。
            点击列标题可按该列排序。模型请求来自用量账本，优先采用来源报告的请求数；
            未报告时每条计费事件计一次，可能少于实际请求数。
          </p>
          <section className="card hairline usage-card">
            <AgentTable rows={data.by_agent} />
          </section>

          <section className="card hairline usage-card">
            {data.by_model.length > 0 ? (
              <ModelTable rows={data.by_model} />
            ) : (
              <p className="usage-note">当前没有任何带模型出处的账本记录。</p>
            )}
            {data.by_model.length > 0 && (
              <p className="usage-note">
                模型的会话成员和会话按参与成员去重，同一会话使用多个模型时会分别计入，不能跨模型相加。
              </p>
            )}
            {data.unattributed_events > 0 && (
              <p className="usage-note">
                另有 {data.unattributed_events.toLocaleString("en-US")} 次计费调用没有模型出处
                （auto 路由、合成通知、格式不记录模型）——它们照常计费，只是归因不到具体模型。
              </p>
            )}
            {ledgerEmpty && <LedgerHint />}
          </section>

          <section className="card hairline usage-card">
            <SortableTable columns={RELATION_COLUMNS} rows={data.by_relation} rowKey={(r) => r.relation} />
          </section>

          {data.by_project.length > 0 && (
            <section className="card hairline usage-card">
              <NamedTable rows={data.by_project} label="项目" onOpen={(projectId) => navigate({ view: "project", projectId })} />
            </section>
          )}

          <section className="card hairline usage-card">
            <NamedTable rows={data.by_workstream} label="任务" />
            {data.by_workstream.length === 0 && (
              <p className="usage-note">
                还没有任务：在会话详情里把会话设为某个任务的 Owner 后，这里会按任务汇总用量。
              </p>
            )}
          </section>

          {data.top_sessions.length > 0 && (
            <section className="card hairline usage-card">
              <TopSessions rows={data.top_sessions} navigate={navigate} />
            </section>
          )}
        </>
      )}
    </div>
  );
}
