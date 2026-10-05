import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import Icon from "../../components/Icon";
import PageHeader from "../../layout/PageHeader";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import SessionMessage, { messageData } from "./SessionMessage";
import { agentDisplayLabel, sessionDisplayTitle, UNTITLED_SESSION } from "./SessionTable";
import type {
  Session,
  SessionMessageMark,
  SessionMessageWindow,
  SessionWindowMessage,
} from "../../types";
import type { Route } from "../../app/routes";

/** 打开时加载的最新一页，也是每次向上/向下滑动加载的量。 */
const PAGE_SIZE = 60;
/** 从导航条跳过去时加载的窗口：比一页大，落点上下都有内容可读。 */
const JUMP_PAGE = 120;
/** 跳转时在目标上方多带多少条：把它落在视口上方三分之一处。 */
const JUMP_LEAD = 40;
/** 距顶部/底部多少像素开始加载下一页。 */
const LOAD_MORE_AT = 240;
/** 距底部多少像素算「在最新处」。 */
const AT_LATEST_SLACK = 24;
/** 滚动停稳多久之后才去取更早的一页：滑动中位置归手势，这一帧写进去的补偿下一帧就被覆盖。 */
const SCROLL_SETTLE_MS = 160;

/** 导航条刻度的固定间距：不按消息位置等比压缩，再长的会话也不会挤成一条线。 */
const RAIL_PITCH = 10;
/** 阶梯首尾各留的边。 */
const RAIL_INSET = 6;
/** 轨道两端渐隐的宽度，也是「高亮那根算不算露在可视区外」的余量。 */
const RAIL_FADE = 24;
/** 阅读线离视口顶部最多这么远。 */
const READING_LINE_MAX = 96;

/** 用投影序号合并两批消息：同一序号只留一条，按序号排序——渲染顺序因此与分页到达的先后无关。 */
function mergeMessages(
  current: SessionWindowMessage[],
  incoming: SessionWindowMessage[],
): SessionWindowMessage[] {
  const byOrdinal = new Map<number, SessionWindowMessage>();
  for (const m of current) byOrdinal.set(m.ordinal, m);
  for (const m of incoming) byOrdinal.set(m.ordinal, m);
  return [...byOrdinal.values()].sort((a, b) => a.ordinal - b.ordinal);
}

/** 行相对滚动区的顶边——与视口在页面上的绝对位置无关，插入前后可直接相减。 */
function flowTop(row: HTMLElement, scroller: HTMLElement): number {
  return row.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
}

/**
 * 视口里某条水平线上那条消息。滚动帧是热路径：先按坐标命中（嵌套元素也能落到正确的行上），
 * 没命中就二分——行本来就是有序的，二分只要 log n 次读，而每次读都要强制一次布局。
 */
function rowAtLine(node: HTMLElement, line: number): HTMLElement | null {
  const box = node.getBoundingClientRect();
  if (typeof document.elementsFromPoint === "function" && box.width > 0) {
    for (const element of document.elementsFromPoint(box.left + box.width / 2, line)) {
      const row = element instanceof HTMLElement ? element.closest<HTMLElement>("[data-seq]") : null;
      if (row !== null && node.contains(row)) return row;
    }
  }
  const rows = node.querySelectorAll<HTMLElement>("[data-seq]");
  let low = 0;
  let high = rows.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (rows[middle].getBoundingClientRect().bottom > line) high = middle;
    else low = middle + 1;
  }
  return rows[low] ?? null;
}

/** 翻页锚点：读者停住的那一行（用 data-seq 认），加它相对滚动区的顶边。 */
interface PagingAnchor {
  sequence: number;
  top: number;
}

function captureAnchor(scroller: HTMLElement | null): PagingAnchor | null {
  if (scroller === null) return null;
  const row = rowAtLine(scroller, scroller.getBoundingClientRect().top);
  if (row === null) return null;
  return { sequence: Number(row.dataset.seq), top: flowTop(row, scroller) };
}

/**
 * 会话消息阅读界面：把整屏交给消息列，从最新一条往前读。
 *
 * 不设条数上限——先只读一页，向上滑按游标往前翻、向下滑向后接上，并锚住读者停住的那一
 * 行；会话被改写时（事实代次变化）旧页不再属于同一个会话，直接回到最新。右侧的导航条
 * 给出一条到用户消息的快速通路。
 */
export default function SessionConversationView({ sessionId, goBack }: {
  sessionId: string;
  goBack: (fallback?: Route) => void;
}) {
  const [session, setSession] = useState<Session | null>(null);
  const [messages, setMessages] = useState<SessionWindowMessage[]>([]);
  const [marks, setMarks] = useState<SessionMessageMark[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [failed, setFailed] = useState(false);
  const [working, setWorking] = useState(false);
  const [atLatest, setAtLatest] = useState(true);
  /** 中间回复（同一轮里代理的非最终输出）默认隐藏；按钮切换为全量模式——
   *  模式切换会按新模式的计页口径重新取页（骨架模式一页 60 条骨架消息）。 */
  const [showIntermediates, setShowIntermediates] = useState(false);
  /** 初始加载 effect 的只读镜像：模式切换不得重触发首屏加载。 */
  const showIntermediatesRef = useRef(showIntermediates);
  showIntermediatesRef.current = showIntermediates;
  /** 当前模式会话尾部的投影序号（模式无关）：「是否已读到尾部」拿它比。 */
  const [tailOrdinal, setTailOrdinal] = useState(0);
  /** 当前模式上方还有多少条没加载（后端按同口径精确计数，跳转后也准确）。 */
  const [remaining, setRemaining] = useState(0);
  /** 每次读到最新一页就自增：滚到底的时机挂在它上面，而不是每次 messages 变化。 */
  const [tailToken, setTailToken] = useState(0);
  /** 跳转落地后要滚到哪一条（投影序号）。 */
  const [pendingJump, setPendingJump] = useState<number | null>(null);
  /** 正在为它翻页的那条用户消息：导航条上那根刻度脉冲。 */
  const [busyMark, setBusyMark] = useState<number | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  /** 正在取一页：同一帧内的重复滚动不能发第二次请求。 */
  const busyRef = useRef(false);
  /** 事实代次，翻页时用来判断旧页是否还属于同一个会话。 */
  const generationRef = useRef<number | null>(null);
  const cursorRef = useRef<number | null>(null);
  /** 读这一页之前读者停住的那一行：插入更早的消息后把它放回原来的位置。 */
  const anchorRef = useRef<PagingAnchor | null>(null);
  /** 已加载范围的第一条投影序号：变小才说明这一帧确实在上方插入了更早的消息。 */
  const firstOrdinalRef = useRef<number | null>(null);
  /** 滚动停稳的定时器：向上翻页要等它响，见 scheduleOlder。 */
  const settleRef = useRef<number | null>(null);

  /** 已加载的范围一直读到了最后一条（拿模式无关的尾部序号比，不拿 total 比）。 */
  const reachesTail = messages.length > 0 && messages[messages.length - 1].ordinal >= tailOrdinal;

  /** 序号 → 投影序号：导航条靠它把「正在读的那一行」对到用户消息刻度上。 */
  const ordinalBySeq = useMemo(
    () => new Map(messages.map((m) => [m.sequence, m.ordinal])),
    [messages],
  );

  /** 页面内容整体替换（不触发「滚到底」）——模式切换用它保住阅读位置。 */
  const applyPage = useCallback((page: SessionMessageWindow) => {
    generationRef.current = page.generation;
    cursorRef.current = page.next_before_ordinal;
    setMessages(page.messages);
    setCursor(page.next_before_ordinal);
    setTotal(page.total);
    setTailOrdinal(page.tail_ordinal);
    setRemaining(page.remaining);
  }, []);

  const applyTail = useCallback((page: SessionMessageWindow) => {
    applyPage(page);
    setTailToken((token) => token + 1);
  }, [applyPage]);

  /** 回到最新一页：首次打开与「跳到最新」都走这条路（按当前模式的计页口径）。 */
  const loadTail = useCallback(async () => {
    anchorRef.current = null;
    applyTail(await api.getSessionMessages(sessionId, { limit: PAGE_SIZE, turnsOnly: !showIntermediates }));
  }, [sessionId, applyTail, showIntermediates]);

  /** 骨架模式 ⇄ 全量模式切换：两种模式的页内容与计数口径都不同，整体重取。 */
  /** 模式切换：按新模式重取一页，并回到切换前正在读的那次提问——位置用导航
   *  条的语言记：视口顶行之前最近的一条用户消息（刻度）。新模式页面以锚点前方
   *  留 JUMP_LEAD 条缓冲的窗口取，保证那次提问落在页内；落地复用跳转机制把它
   *  滚到视口上方。像素级恢复抗不住重渲染后的行高漂移，刻度是稳定的位置坐标。 */
  const switchMode = useCallback((show: boolean) => {
    if (show === showIntermediates) return;
    setShowIntermediates(show);
    void (async () => {
      try {
        const anchorRow = captureAnchor(scrollRef.current);
        const anchorOrdinal = anchorRow === null
          ? null
          : messages.find((m) => m.sequence === anchorRow.sequence)?.ordinal ?? null;
        // 先取 marks 再取页：锚点换算要用目标模式的用户消息刻度（markList 与
        // 切换前的 marks 内容一致，同批取回避免用旧值）。
        const markList = await api.getSessionUserMessageMarks(sessionId);
        // 视口顶行之前最近的一次提问；顶行之前没有（还在第一问上方）就用第一问。
        const targetOrdinal = anchorOrdinal === null
          ? null
          : [...markList].reverse().find((mark) => mark.ordinal <= anchorOrdinal)?.ordinal
            ?? markList[0]?.ordinal ?? null;
        const page = await (anchorOrdinal === null
          ? api.getSessionMessages(sessionId, { limit: PAGE_SIZE, turnsOnly: !show })
          : api.getSessionMessages(sessionId, {
              beforeOrdinal: anchorOrdinal + JUMP_LEAD + 1,
              limit: JUMP_PAGE,
              turnsOnly: !show,
            }));
        if (page.generation !== generationRef.current) {
          // 会话在切换途中被改写：旧位置已无意义，回最新。
          showToast("这个会话已被改写，已回到最新");
          applyTail(page);
          setMarks(markList);
          return;
        }
        anchorRef.current = null;
        applyPage(page);
        setMarks(markList);
        if (targetOrdinal === null) {
          // 没有可锚的提问（空会话、导航条无刻度）：停在新页当前位置，不跳底。
          return;
        }
        if (!page.messages.some((m) => m.ordinal === targetOrdinal)) {
          // 理论上窗口保证那次提问在页内；万一不在，停在页顶而不是回尾。
          return;
        }
        // 落地由 pendingJump 的 effect 统一滚到那次提问。
        setPendingJump(targetOrdinal);
      } catch (error) {
        console.error(error);
        showToast(String(error));
      }
    })();
  }, [sessionId, showIntermediates, messages, marks, applyPage, applyTail]);

  useEffect(() => {
    let live = true;
    setFailed(false);
    void Promise.all([
      api.getSessionDetail(sessionId),
      api.getSessionMessages(sessionId, { limit: PAGE_SIZE, turnsOnly: !showIntermediatesRef.current }),
      api.getSessionUserMessageMarks(sessionId),
    ])
      .then(([detail, page, markList]) => {
        if (!live) return;
        setSession(detail.session);
        setMarks(markList);
        applyTail(page);
      })
      .catch((error) => {
        console.error(error);
        if (live) setFailed(true);
      });
    return () => { live = false; };
    // 模式切换不得重触发首屏加载（否则 applyTail 会把读者甩回底部）；模式经 ref 读。
  }, [sessionId, applyTail]);

  /** 打开、重新读取、以及跳回最新，都停在最新一条上。 */
  useLayoutEffect(() => {
    const node = scrollRef.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
    setAtLatest(true);
  }, [tailToken]);

  /**
   * 插入更早的消息后，把读者停住的那一行放回它原来的位置。
   *
   * 判据是新首页的投影序号比上一帧小——只在「这一帧确实插入了更早的消息」时动手，尾部
   * 追加、按钮换字这类无关的高度变化不会动视口；补偿量取自那一行自己的坐标而不是内容
   * 高度差，插入引起的其它高度变化也污染不到它。`.conversation-scroll` 关掉了引擎的原生
   * 锚定，这里是唯一的补偿方。
   */
  useLayoutEffect(() => {
    const node = scrollRef.current;
    const anchor = anchorRef.current;
    const first = messages.length === 0 ? null : messages[0].ordinal;
    const previous = firstOrdinalRef.current;
    firstOrdinalRef.current = first;
    anchorRef.current = null;
    if (node === null || anchor === null || first === null || previous === null || first >= previous) return;
    const row = node.querySelector<HTMLElement>(`[data-seq="${anchor.sequence}"]`);
    if (row === null) return;
    node.scrollTop += flowTop(row, node) - anchor.top;
  }, [messages]);

  /**
   * 跳转落地：把目标那条滚到视口上方，然后按目标行做稳定锚定。
   *
   * 一帧的 scrollToSeq 不够：全窗替换落地时 Markdown 还没渲染，目标行上方的
   * 几十行各自要继续撑高，每高一行就把目标行往下推一截——读者看到的落点比预期
   * 偏下且偏多少不定。所以落地后盯住目标行的 flowTop，渲染把它挤走就立刻拉回
   * 16px，连续两帧无漂移才算站稳（上限 20 次防 Markdown 无限布局的极端情况）。
   */
  useLayoutEffect(() => {
    if (pendingJump === null) return;
    const target = messages.find((m) => m.ordinal === pendingJump);
    setPendingJump(null);
    const node = scrollRef.current;
    if (!target || node === null) return;
    const seq = target.sequence;
    const HOLD_AT = 16;
    let pin = node.scrollTop + flowTop(
      node.querySelector<HTMLElement>(`[data-seq="${seq}"]`)!, node,
    ) - HOLD_AT;
    node.scrollTop = pin;
    let settled = 0;
    let frames = 0;
    const step = () => {
      const row = node.querySelector<HTMLElement>(`[data-seq="${seq}"]`);
      if (row === null) return; // 目标被改写清走：放弃
      const drift = node.scrollTop + flowTop(row, node) - HOLD_AT - pin;
      if (Math.abs(drift) > 0.5) {
        node.scrollTop -= drift;
        settled = 0;
      } else {
        settled += 1;
      }
      frames += 1;
      if (settled < 2 && frames < 20) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }, [messages, pendingJump]);

  /** 滚动停稳之后再去取更早的一页（读者点按钮是明确动作，那个直接调 loadOlder）。 */
  const scheduleOlder = () => {
    if (settleRef.current !== null) window.clearTimeout(settleRef.current);
    settleRef.current = window.setTimeout(() => {
      settleRef.current = null;
      const node = scrollRef.current;
      if (node !== null && node.scrollTop <= LOAD_MORE_AT) void loadOlder();
    }, SCROLL_SETTLE_MS);
  };

  useEffect(() => () => {
    if (settleRef.current !== null) window.clearTimeout(settleRef.current);
  }, []);

  const loadOlder = async () => {
    const before = cursorRef.current;
    if (before === null || busyRef.current) return;
    busyRef.current = true;
    setWorking(true);
    try {
      const page = await api.getSessionMessages(sessionId, { beforeOrdinal: before, limit: PAGE_SIZE, turnsOnly: !showIntermediates });
      if (page.generation !== generationRef.current) {
        showToast("这个会话已被改写，已回到最新");
        await loadTail();
        return;
      }
      // 游标在请求落地时就前进：期间重入的滚动事件不会再用同一个游标取一次同样的页。
      cursorRef.current = page.next_before_ordinal;
      // 锚点现取：读者可能在请求期间又往上滑了。
      anchorRef.current = captureAnchor(scrollRef.current);
      setMessages((previous) => mergeMessages(previous, page.messages));
      setCursor(page.next_before_ordinal);
      setTotal(page.total);
      setRemaining(page.remaining);
    } catch (error) {
      console.error(error);
      showToast(String(error));
    } finally {
      busyRef.current = false;
      setWorking(false);
    }
  };

  /** 向下的续读：跳进中段之后，往下读还要能接上更新的消息。 */
  const loadNewer = async () => {
    const last = messages[messages.length - 1];
    if (!last || busyRef.current || reachesTail) return;
    busyRef.current = true;
    setWorking(true);
    try {
      const page = await api.getSessionMessages(sessionId, { afterOrdinal: last.ordinal, limit: PAGE_SIZE, turnsOnly: !showIntermediates });
      if (page.generation !== generationRef.current) {
        showToast("这个会话已被改写，已回到最新");
        await loadTail();
        return;
      }
      // 追加在下方：首页没变，锚点那一关就不会过，视口不动。
      setMessages((previous) => mergeMessages(previous, page.messages));
      setTotal(page.total);
    } catch (error) {
      console.error(error);
      showToast(String(error));
    } finally {
      busyRef.current = false;
      setWorking(false);
    }
  };

  /** 导航条跳转：已加载的直接滚过去，没加载的先取一段以它为中心的窗口。 */
  const jumpToMark = async (ordinal: number) => {
    if (busyRef.current) return;
    const loaded = messages.find((m) => m.ordinal === ordinal);
    if (loaded) {
      scrollToSeq(scrollRef.current, loaded.sequence, 16);
      return;
    }
    busyRef.current = true;
    setWorking(true);
    setBusyMark(ordinal);
    try {
      const page = await api.getSessionMessages(sessionId, {
        beforeOrdinal: ordinal + JUMP_LEAD + 1,
        limit: JUMP_PAGE,
        turnsOnly: !showIntermediates,
      });
      if (page.generation !== generationRef.current) {
        showToast("这个会话已被改写，已回到最新");
        await loadTail();
        return;
      }
      cursorRef.current = page.next_before_ordinal;
      // 整窗替换：新首页完全可能是更早的序号，这里不留下会让锚点误判的旧锚。
      anchorRef.current = null;
      setMessages(page.messages);
      setCursor(page.next_before_ordinal);
      setTotal(page.total);
      setRemaining(page.remaining);
      setPendingJump(ordinal);
    } catch (error) {
      console.error(error);
      showToast(String(error));
    } finally {
      busyRef.current = false;
      setWorking(false);
      setBusyMark(null);
    }
  };

  const onScroll = () => {
    const node = scrollRef.current;
    if (!node) return;
    const toBottom = node.scrollHeight - node.scrollTop - node.clientHeight;
    setAtLatest(toBottom <= AT_LATEST_SLACK && reachesTail);
    // 向下续读是往下方追加，不会挪动读者正看的内容，立刻取没有风险。
    if (toBottom <= LOAD_MORE_AT && !reachesTail) void loadNewer();
    if (node.scrollTop <= LOAD_MORE_AT) scheduleOlder();
  };

  /** 「跳到最新」：没读到尾部就重载尾部窗口，否则滚到底。 */
  const jumpToLatest = () => {
    if (!reachesTail) {
      void loadTail().catch((error) => {
        console.error(error);
        showToast(String(error));
      });
      return;
    }
    const node = scrollRef.current;
    node?.scrollTo({ top: node.scrollHeight, behavior: "smooth" });
  };

  if (failed) {
    return (
      <div className="main narrow" role="status">
        <PageHeader back="返回会话详情" onBack={() => goBack()} title="会话消息" />
        <div className="empty">
          读不到这个会话的消息。
          <div className="small" style={{ marginTop: 4 }}>返回详情页后重试。</div>
        </div>
      </div>
    );
  }

  const title = session === null ? "会话消息" : sessionDisplayTitle(session.title);
  const untitled = title === UNTITLED_SESSION;

  return (
    <div className="main narrow fill">
      <PageHeader
        back="返回会话详情"
        onBack={() => goBack()}
        title={untitled ? "未命名会话" : title}
        sub={session && <>{agentDisplayLabel(session.agent)} · 共 {total} 条消息</>}
        actions={(
          <span className="row" style={{ gap: 8 }}>
            <button
              className={`btn ghost icon-button${!showIntermediates ? " on" : ""}`}
              aria-label={showIntermediates ? "隐藏中间回复" : "显示中间回复"}
              aria-pressed={!showIntermediates}
              title={!showIntermediates
                ? "当前只看用户消息和每轮的最终回复；点这里显示代理的中间输出"
                : "当前显示全部消息；点这里隐藏代理的中间输出"}
              onClick={() => switchMode(!showIntermediates)}
            >
              <Icon name="filter" />
            </button>
            <button
              className="btn ghost icon-button"
              aria-label="重新读取"
              title="回到最新一条"
              onClick={() => {
                void Promise.all([loadTail(), api.getSessionUserMessageMarks(sessionId).then(setMarks)])
                  .catch((error) => {
                    console.error(error);
                    showToast(String(error));
                  });
              }}
            >
              <Icon name="refresh" />
            </button>
          </span>
        )}
      />

      <div className="conversation-body">
        <div className="conversation-scroll" ref={scrollRef} onScroll={onScroll}>
          {cursor !== null && (
            <div className="conversation-more">
              <button className="btn small ghost" disabled={working} onClick={() => void loadOlder()}>
                {working ? "正在加载…" : `加载更早的消息（还有 ${remaining} 条）`}
              </button>
            </div>
          )}
          {messages.length === 0 ? (
            <div className="empty">
              还没有摄入消息。
              <div className="small" style={{ marginTop: 4 }}>
                这个会话的来源里没有可读的 user / assistant 消息。
              </div>
            </div>
          ) : (
            messages.map((m) => <SessionMessage key={m.sequence} msg={messageData(m, session?.agent ?? null)} />)
          )}
        </div>
        {marks.length > 0 && (
          <ConversationNav
            scrollerRef={scrollRef}
            marks={marks}
            ordinalBySeq={ordinalBySeq}
            busyMark={busyMark}
            onJump={(ordinal) => void jumpToMark(ordinal)}
          />
        )}
      </div>

      {!atLatest && (
        <button className="btn small conversation-jump" onClick={jumpToLatest}>跳到最新 ↓</button>
      )}
    </div>
  );
}

/** 把某条消息滚到视口顶部（留一点余量）。 */
function scrollToSeq(node: HTMLDivElement | null, seq: number, margin: number) {
  const row = node?.querySelector<HTMLElement>(`[data-seq="${seq}"]`);
  if (!node || !row) return;
  node.scrollTop += flowTop(row, node) - margin;
}

/**
 * 用户消息导航条：一条用户消息一根短横，按固定间距排成阶梯。
 *
 * 刻度位置只由它在用户消息里的序号决定，不映射到消息在文档里的位置——它是「跳到第几条
 * 提问」的目录，不是滚动条的替身。所以刻度永不重叠、不需要合并，阶梯高过上限时也是轨道
 * 自己滚。滚动监听留在这里，滚动只重渲染这几根刻度，不牵动上面几百条消息。
 */
function ConversationNav({ scrollerRef, marks, ordinalBySeq, busyMark, onJump }: {
  scrollerRef: React.RefObject<HTMLDivElement | null>;
  marks: SessionMessageMark[];
  /** 已加载消息的 序号 → 投影序号。 */
  ordinalBySeq: Map<number, number>;
  /** 正在为它翻页的刻度。 */
  busyMark: number | null;
  onJump: (ordinal: number) => void;
}) {
  const [active, setActive] = useState<number | null>(null);
  const [railEnds, setRailEnds] = useState({ up: false, down: false });
  const railRef = useRef<HTMLDivElement | null>(null);

  const loaded = useMemo(() => new Set(ordinalBySeq.values()), [ordinalBySeq]);
  const naturalHeight = Math.max((marks.length - 1) * RAIL_PITCH + 2 * RAIL_INSET, 0);

  /** 阅读线落在哪一行的那条消息，再取不晚于它的最后一条用户消息当「正在读的」。 */
  const syncActive = useCallback(() => {
    const node = scrollerRef.current;
    if (node === null || marks.length === 0) return;
    // 贴在最底下时高亮最后一条：阅读线那时落在更早的行上，读者却已经读完了。
    if (node.scrollHeight - node.scrollTop - node.clientHeight <= AT_LATEST_SLACK) {
      const latest = marks[marks.length - 1].ordinal;
      setActive((current) => (current === latest ? current : latest));
      return;
    }
    const box = node.getBoundingClientRect();
    const row = rowAtLine(node, box.top + Math.min(READING_LINE_MAX, box.height * 0.2));
    const reading = row === null ? undefined : ordinalBySeq.get(Number(row.dataset.seq));
    let next = marks[0].ordinal;
    if (reading !== undefined) {
      for (const mark of marks) {
        if (mark.ordinal > reading) break;
        next = mark.ordinal;
      }
    }
    setActive((current) => (current === next ? current : next));
  }, [marks, ordinalBySeq, scrollerRef]);

  const syncRailScroll = useCallback(() => {
    const rail = railRef.current;
    if (rail === null) return;
    const next = {
      up: rail.scrollTop > 1,
      down: rail.scrollTop < rail.scrollHeight - rail.clientHeight - 1,
    };
    setRailEnds((current) => (current.up === next.up && current.down === next.down ? current : next));
  }, []);

  useEffect(() => {
    const node = scrollerRef.current;
    if (node === null) return;
    syncActive();
    node.addEventListener("scroll", syncActive);
    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(syncActive);
      observer.observe(node);
    }
    return () => {
      node.removeEventListener("scroll", syncActive);
      observer?.disconnect();
    };
  }, [scrollerRef, syncActive]);

  /** 高亮那条要留在轨道可视范围内，否则阶梯一长就看不见自己读到哪了。 */
  useEffect(() => {
    const rail = railRef.current;
    const index = marks.findIndex((mark) => mark.ordinal === active);
    if (rail === null || index < 0) return;
    const markTop = index * RAIL_PITCH + RAIL_INSET;
    const viewHeight = rail.clientHeight;
    if (viewHeight <= 0 || (markTop >= rail.scrollTop + RAIL_FADE && markTop <= rail.scrollTop + viewHeight - RAIL_FADE)) return;
    const target = Math.max(0, markTop - viewHeight / 2);
    const reduced = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (typeof rail.scrollTo === "function") rail.scrollTo({ top: target, behavior: reduced ? "auto" : "smooth" });
    else rail.scrollTop = target;
    syncRailScroll();
  }, [active, marks, syncRailScroll]);

  const fade = ["nav-scroll", railEnds.up ? "fade-up" : "", railEnds.down ? "fade-down" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <nav
      className="conversation-nav"
      aria-label="用户消息"
      style={{ "--nav-natural-height": `${naturalHeight}px` } as React.CSSProperties}
    >
      <div ref={railRef} className={fade} onScroll={syncRailScroll}>
        <div className="nav-marks">
          {marks.map((mark, index) => {
            const label = mark.preview || `第 ${mark.ordinal} 条消息`;
            const isActive = mark.ordinal === active;
            const isBusy = mark.ordinal === busyMark;
            const classes = ["nav-tick"];
            if (!loaded.has(mark.ordinal)) classes.push("is-unloaded");
            if (isActive) classes.push("is-active");
            if (isBusy) classes.push("is-busy");
            return (
              <div key={mark.ordinal} className="nav-mark" style={{ top: index * RAIL_PITCH + RAIL_INSET }}>
                <button
                  type="button"
                  className={classes.join(" ")}
                  title={label}
                  aria-label={label}
                  aria-current={isActive ? "true" : undefined}
                  aria-busy={isBusy ? "true" : undefined}
                  onClick={() => onJump(mark.ordinal)}
                />
              </div>
            );
          })}
        </div>
      </div>
    </nav>
  );
}
