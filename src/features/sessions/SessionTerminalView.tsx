import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import PageHeader from "../../layout/PageHeader";
import SessionSubpageTabs from "./SessionSubpageTabs";
import { DesktopContinueButton } from "./SessionHeaderActions";
import { preferredTerminal, terminalDestination } from "./sessionNavigation";
import { copyToClipboard } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { sessionDisplayTitle } from "./SessionTable";
import type { Agent, TerminalBound, TerminalSnapshot } from "../../types";
import { EVT_TERMINALS, type Route } from "../../app/routes";
import { liveTerminals, disposeTerminal } from "./terminalCache";
import { observeTerminalTheme } from "./terminalTheme";
import { connectTerminalInput } from "./terminalInput";
import { connectWindowsClipboard, isWindowsTerminal } from "./terminalWindows";
import { connectTerminalLayout } from "./terminalLayout";
import { connectTerminalViewport } from "./terminalViewport";
import { connectWindowsComposition } from "./terminalComposition";
import { createTerminalOutput } from "./terminalOutput";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { connectTerminalKeyboard, isMacTerminal } from "./terminalKeyboard";
import { terminalOptions, createTerminalRenderer, attachTerminalLayout } from "./terminalPresentation";
import { connectTerminalLinks } from "./terminalLinks";
import { subscribeTerminalStream, type TerminalStream } from "./terminalStream";

/**
 * 会话页的内嵌终端面板。路由保留 terminal_id 以访问尚未绑定的实例；PTY 与 scrollback 都归后端（terminal
 * registry），本组件只是 attach/detach 客户端——切走再切回（remount）进程
 * 照跑，xterm 实例按 terminal_id 常驻，重挂零重建。
 *
 * 当前会话身份由后端从 Agent 的原生会话 ID 确认。CLI 内切换会话时，
 * `terminal-bound` 推送新版身份（尚未摄入时为 null），视图随之更新标题与
 * 详情入口。身份版本阻止迟到快照或详情请求把界面退回旧会话。
 *
 * 已知的产品边界：关闭 NoEnding 会结束后端里的内嵌 Agent（后端 RunEvent::Exit
 * 统一收割）。
 */

type TerminalState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ready"; snapshot: TerminalSnapshot; exited: boolean; stream: TerminalStream | null };

/** 注册表的身份广播：绑定、切换与 pending 清空都走同一个版本化通道。 */
const EVENT_BOUND = "terminal-bound";

type BoundIdentity = {
  revision: number;
  sessionId: string | null;
  title: string | null;
};

/** 测试钩子：模块级注册表会跨用例存活。 */
export function resetLiveTerminalsForTests(): void {
  for (const id of liveTerminals.keys()) disposeTerminal(id);
}

export default function SessionTerminalView({ terminalId, initialTitle, initialAgent, initialSessionId, navigate }: {
  /** 独立终端路由：内嵌新建与「先跳后启」的共同落点。 */
  terminalId: string;
  /** 会话页跳转带来的首帧占位；随后由版本化后端身份替换。 */
  initialTitle?: string;
  initialAgent?: Agent;
  initialSessionId?: string;
  navigate: (r: Route) => void;
}) {
  const [state, setState] = useState<TerminalState>({ kind: "loading" });
  const containerRef = useRef<HTMLDivElement | null>(null);
  // 路由种子只填首帧；任何权威快照/事件（包括 null）都会替换它。
  const [identity, setIdentity] = useState<BoundIdentity>({
    revision: -1,
    sessionId: initialSessionId ?? null,
    title: initialTitle ?? null,
  });
  const identityRef = useRef(identity);
  const boundSessionId = identity.sessionId;
  const boundTitle = identity.title;
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const [reconnecting, setReconnecting] = useState(false);
  const [closing, setClosing] = useState(false);
  const closingRef = useRef(false);
  const [sessionMeta, setSessionMeta] = useState<{ id: string; sourceKind: string; archived: boolean } | null>(null);
  const reconnectingRef = useRef(false);
  const mountedRef = useRef(true);
  useEffect(() => { mountedRef.current = true; return () => { mountedRef.current = false; }; }, []);

  const applyIdentity = useCallback((update: TerminalBound & { session_title?: string | null }, fromSnapshot = false) => {
    const current = identityRef.current;
    if (update.identity_revision < current.revision) return;
    // 同版本广播是重复通知；同版本快照可补齐权威标题，但不能改变身份。
    if (update.identity_revision === current.revision && (!fromSnapshot || update.session_id !== current.sessionId)) return;
    let title: string | null = null;
    if (update.session_id) {
      if (update.session_title != null) title = sessionDisplayTitle(update.session_title);
      else if (update.session_id === current.sessionId) title = current.title;
    }
    const next = { revision: update.identity_revision, sessionId: update.session_id, title };
    identityRef.current = next;
    setIdentity(next);
  }, []);

  /** 重新连接会按后端当前绑定身份恢复会话，并切换到新 PTY。 */
  const reconnectTerminal = async () => {
    if (reconnectingRef.current || !boundSessionId) return;
    reconnectingRef.current = true;
    setReconnecting(true);
    try {
      const result = await api.terminalReconnect(terminalId);
      if (!result.terminal_id) throw new Error("找不到重新连接的终端");
      // 旧实例已被后端替换，释放对应 xterm 与常驻监听。
      disposeTerminal(terminalId);
      if (mountedRef.current) navigate({ view: "terminal", terminalId: result.terminal_id, initialTitle: boundTitle ?? undefined, initialAgent: snapshot?.agent ?? initialAgent, initialSessionId: boundSessionId });
    } catch (e) {
      if (mountedRef.current) showToast(`重新连接失败：${String(e)}`);
    } finally {
      reconnectingRef.current = false;
      if (mountedRef.current) setReconnecting(false);
    }
  };

  const closeTerminal = async () => {
    if (closingRef.current || reconnectingRef.current) return;
    closingRef.current = true;
    setClosing(true);
    try {
      const closed = await api.terminalClose(terminalId);
      disposeTerminal(terminalId);
      window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
      let remaining: import("../../types").TerminalSummary[] = [];
      try { remaining = await api.terminalList(); } catch { /* Closing succeeded; still leave the removed terminal. */ }
      if (!mountedRef.current) return;
      const peers = remaining.filter(t => t.terminal_id !== terminalId && t.session_id === closed.session_id && !!closed.session_id);
      const next = closed.session_id ? preferredTerminal(closed.session_id, peers.filter(t => t.live)) ?? peers[0] : undefined;
      navigate(next ? terminalDestination(next)
        : closed.session_id ? { view: "session", sessionId: closed.session_id, ...(closed.session_id === boundSessionId ? { initialTitle: boundTitle ?? undefined, initialAgent: snapshot?.agent ?? initialAgent } : {}) }
        : { view: "sessions" });
    } catch (e) { if (mountedRef.current) showToast(`关闭失败：${String(e)}`); }
    finally { closingRef.current = false; if (mountedRef.current) setClosing(false); }
  };

  /** 进入即用：按 terminalId 直接 attach——终端在新会话发送或「先跳后启」
   *  启动时就已存在，本视图从不负责 spawn。 */
  const bootstrap = () => setConnectionAttempt((attempt) => attempt + 1);

  // 身份、输出与退出都先完成订阅，再读快照。输出按字节位置去掉快照重叠。
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    setState({ kind: "loading" });
    const identityReady = listen<TerminalBound>(EVENT_BOUND, (e) => {
      if (active && e.payload.terminal_id === terminalId) applyIdentity(e.payload);
    }).then(stop => {
      if (!active) stop();
      else unlisten = stop;
    });
    // 缓存实例已经持有常驻输出订阅，重挂时不能再订阅或重放。
    const cached = liveTerminals.get(terminalId);
    const stream = cached ? null : subscribeTerminalStream(terminalId,
      isWindowsTerminal() ? {
        acknowledge: (offset, consumerId) => api.terminalAcknowledgeOutput(terminalId, offset, consumerId),
        release: consumerId => api.terminalReleaseOutput(terminalId, consumerId),
      } : undefined);
    void Promise.all([identityReady, stream?.ready])
      .then(async () => {
        if (!active) return;
        const snapshot = await (stream
          ? stream.attach(consumerId => consumerId ? api.terminalAttach(terminalId, consumerId) : api.terminalAttach(terminalId))
          : api.terminalAttach(terminalId));
        if (!active) return;
        applyIdentity(snapshot, true);
        setState({ kind: "ready", snapshot, stream, exited: !snapshot.live || !!cached?.exited || !!stream?.exited });
      })
      .catch((error) => {
        stream?.dispose();
        if (active) setState({ kind: "error", message: String(error) });
      });
    return () => {
      active = false;
      unlisten?.();
      if (!stream?.retained) stream?.dispose();
    };
  }, [terminalId, connectionAttempt, applyIdentity]);

  // 读取当前会话的格式、归档状态和标题；版本检查防住 A→B→A 的迟到请求。
  useEffect(() => {
    if (!identity.sessionId || identity.revision < 0) return;
    const { sessionId, revision } = identity;
    let live = true;
    api.getSessionDetail(sessionId)
      .then((d) => {
        const current = identityRef.current;
        if (!live || current.revision !== revision || current.sessionId !== sessionId) return;
        setSessionMeta({ id: sessionId, sourceKind: d.session.source_kind, archived: !!d.session.archived_at });
        if (current.title !== null) return;
        const next = { ...current, title: sessionDisplayTitle(d.session.title) };
        identityRef.current = next;
        setIdentity(next);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [identity.sessionId, identity.revision]);

  // xterm 生命周期：实例按 terminal_id 常驻（liveTerminals），本 effect 只做
  // 两件事——首次创建（open + 监听 + 回放一次 scrollback），或重挂（把存活的
  // DOM 节点搬回新容器）。unmount 只断开 ResizeObserver 和横幅联动，不销毁
  // 实例；PTY 输出在 detach 期间继续写入同一实例。
  const snapshot = state.kind === "ready" ? state.snapshot : null;
  const stream = state.kind === "ready" ? state.stream : null;
  const exited = state.kind === "ready" && state.exited;
  useEffect(() => {
    if (snapshot === null || containerRef.current === null) return;
    const id = snapshot.terminal_id;
    const container = containerRef.current;

    let entry = liveTerminals.get(id);
    if (!entry) {
      if (!stream) return;
      const windows = isWindowsTerminal();
      const term = new Terminal({ ...terminalOptions(isMacTerminal()), ...(windows ? {
        cols: snapshot.cols, rows: snapshot.rows,
        fontFamily: "Consolas, 'Courier New', monospace", fontSize: 14,
        cursorBlink: false, minimumContrastRatio: 1, allowProposedApi: true,
      } : {}) });
      const stopTheme = observeTerminalTheme(term, (data) => {
        if (!snapshot.live || liveTerminals.get(id)?.exited) return;
        void api.terminalInput(id, data).catch(() => {});
      }, windows);
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(container);
      let stopViewport = () => {};
      let stopComposition = () => {};
      let output: ReturnType<typeof createTerminalOutput> | undefined;
      const renderer = createTerminalRenderer(term);
      if (windows) {
        term.loadAddon(new Unicode11Addon());
        term.unicode.activeVersion = "11";
        stopViewport = connectTerminalViewport(term);
        stopComposition = connectWindowsComposition(term, () => api.terminalInputCursor(id));
        output = createTerminalOutput(term);
        term.textarea?.setAttribute("aria-label", "终端输入；F6 返回页面操作");
      }
      const stopLinks = connectTerminalLinks(term, isMacTerminal(), api.openRemoteUrl);
      const stopKeyboard = windows ? connectWindowsClipboard(term, {
        read: api.readClipboardForTerminal, copy: api.writeClipboardForTerminal,
        live: () => !liveTerminals.get(id)?.exited && liveTerminals.has(id),
      }) : connectTerminalKeyboard(term, {
        mac: isMacTerminal(),
        writable: () => !liveTerminals.get(id)?.exited && liveTerminals.has(id),
        readClipboard: api.readClipboardForTerminal,
        copy: copyToClipboard,
      });

      // 输入通道随实例常驻：detach 期间没有 UI 焦点自然无输入，无需重建。
      const stopInput = connectTerminalInput(term, (data, binary) => {
        const liveNow = liveTerminals.get(id);
        if (!liveNow || liveNow.exited) return;
        api.terminalInput(id, data, binary).catch(() => {
          // 已退出的终端拒绝输入：横幅已说明，不再打扰。
        });
      }, windows);

      entry = {
        term,
        fit,
        renderer,
        dimensions: { cols: snapshot.cols, rows: snapshot.rows },
        unlisteners: [stopTheme, stopInput, stopKeyboard, stopLinks, stopViewport, stopComposition, () => output?.dispose(), () => renderer.dispose(), () => stream.dispose()],
        exited: !snapshot.live || stream.exited,
        onExit: null,
      };
      liveTerminals.set(id, entry);
      stream.replay(snapshot, (data, done) => output ? output.write(data, done) : term.write(data, done), () => {
        const live = liveTerminals.get(id);
        if (live) {
          live.exited = true;
          live.onExit?.();
        }
      });
    } else {
      // 重挂：搬回节点，不回放、不新建。状态（含 detach 期间的输出与退出
      // 事实）都在实例里。
      if (entry.term.element) container.appendChild(entry.term.element);
    }
    const live = entry;

    // 横幅与实例的退出事实同步：挂载时读一次，之后由 exit 事件推送。
    setState((prev) =>
      prev.kind === "ready" && prev.snapshot.terminal_id === id
        ? { ...prev, exited: live.exited }
        : prev,
    );
    live.onExit = () => {
      setState((prev) =>
        prev.kind === "ready" && prev.snapshot.terminal_id === id
          ? { ...prev, exited: true }
          : prev,
      );
    };

    if (isWindowsTerminal()) {
      const stopLayout = connectTerminalLayout(live.term, live.fit, container,
        (cols, rows) => live.exited ? Promise.resolve() : api.terminalResize(id, cols, rows), snapshot);
      return () => { live.onExit = null; live.term.element?.dispatchEvent(new Event("terminal-view-detached")); stopLayout(); };
    }

    const layout = attachTerminalLayout(live.term, live.fit, container, (cols, rows) => {
      if (live.exited || (cols === live.dimensions.cols && rows === live.dimensions.rows)) return;
      const previous = live.dimensions;
      const next = { cols, rows };
      live.dimensions = next;
      void api.terminalResize(id, cols, rows).catch(() => {
        if (live.dimensions === next) live.dimensions = previous;
      });
    });
    live.renderer.onDimensionsChange = layout.refresh;
    layout.refresh();

    return () => {
      live.onExit = null;
      live.renderer.onDimensionsChange = null;
      layout.dispose();
    };
  }, [snapshot, stream]);

  // 未绑定的终端以「新会话」为名；绑定后标题换成会话本名。agent 在
  // attach 回来前用路由种子，避免首帧丢失图标。
  const headerTitle = boundTitle ?? (boundSessionId ? "" : "新会话");
  const headerAgent = snapshot?.agent ?? initialAgent ?? null;
  const meta = sessionMeta?.id === boundSessionId ? sessionMeta : null;

  return (
    <div className="main fill">
      <PageHeader
        title={
          headerAgent ? (
            <span className="session-title-with-icon" title={headerTitle}>
              <AgentIcon agent={headerAgent} size={18} />
              <span className="session-title-text">{headerTitle}</span>
            </span>
          ) : (
            headerTitle
          )
        }
        actions={
          <>
            <SessionSubpageTabs sessionId={boundSessionId} title={headerTitle} terminalId={terminalId} terminalLive={state.kind === "ready" ? !exited : undefined}
              agent={headerAgent ?? undefined} sourceKind={meta?.sourceKind} archived={meta?.archived} navigate={navigate} />
            {boundSessionId && headerAgent && <DesktopContinueButton sessionId={boundSessionId} agent={headerAgent}
              sourceKind={meta?.sourceKind} archived={meta?.archived ?? false} />}
            <button
              className="btn ghost icon-button"
              aria-label={reconnecting ? "正在重新连接" : "重新连接"}
              title={meta?.archived ? "已归档的会话不能重新连接" : boundSessionId ? "重新连接会话" : "绑定会话后可重新连接"}
              disabled={closing || reconnecting || !boundSessionId || !!meta?.archived || state.kind === "loading"}
              onClick={() => void reconnectTerminal()}
            >
              <Icon name="refresh" />
            </button>
            <button className="btn ghost icon-button" aria-label={exited ? "移除终端" : "关闭终端"}
              title={exited ? "移除终端" : "关闭当前终端"} disabled={closing || reconnecting || state.kind === "loading"}
              onClick={() => void closeTerminal()}><Icon name="close" /></button>
          </>
        }
      />

      {state.kind === "loading" && <div className="empty">正在连接内嵌终端…</div>}
      {state.kind === "error" && (
        <div className="empty">
          内嵌终端不可用。
          <div className="small" style={{ marginTop: 4 }}>{state.message}</div>
          <div style={{ marginTop: 12 }}>
            <button className="btn small" onClick={bootstrap}>重试</button>
          </div>
        </div>
      )}
      {snapshot !== null && (
        <>
          {snapshot.live && !boundSessionId && (
            <div className="terminal-exit-banner" role="status">
              Agent 运行中。绑定会话后可查看详情。
            </div>
          )}
          {exited && (
            <div className="terminal-exit-banner" role="status">
              Agent 已退出
              {snapshot.exit_code !== null && <>（退出码 {snapshot.exit_code}）</>}。
            </div>
          )}
          <div className="terminal-body">
            <div className={`terminal-host${isMacTerminal() ? " terminal-mac" : ""}`} data-platform={isWindowsTerminal() ? "windows" : isMacTerminal() ? "mac" : "other"} ref={containerRef} />
          </div>
        </>
      )}
    </div>
  );
}
