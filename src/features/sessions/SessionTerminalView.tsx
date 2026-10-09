import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import PageHeader from "../../layout/PageHeader";
import { copyToClipboard } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { sessionDisplayTitle } from "./SessionTable";
import type { Agent, TerminalBound, TerminalSnapshot } from "../../types";
import type { Route } from "../../app/routes";
import { liveTerminals, disposeTerminal } from "./terminalCache";
import { observeTerminalTheme } from "./terminalTheme";

/**
 * 内嵌终端视图（一等独立路由）。PTY 与 scrollback 都归后端（terminal
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

/** base64 → PTY 原始字节。xterm.write 直接吃 Uint8Array，不经过字符串化。 */
function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/** 裸修饰键（e.key）：IME 首键直提交时唯一能先到的 keydown，武装差分窗口用。 */
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta"]);

type TerminalState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ready"; snapshot: TerminalSnapshot; exited: boolean };

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
      else if (update.identity_revision === current.revision) title = current.title;
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
      if (mountedRef.current) navigate({ view: "terminal", terminalId: result.terminal_id });
    } catch (e) {
      if (mountedRef.current) showToast(`重新连接失败：${String(e)}`);
    } finally {
      reconnectingRef.current = false;
      if (mountedRef.current) setReconnecting(false);
    }
  };

  /** 进入即用：按 terminalId 直接 attach——终端在新会话发送或「先跳后启」
   *  启动时就已存在，本视图从不负责 spawn。 */
  const bootstrap = () => setConnectionAttempt((attempt) => attempt + 1);

  // 先完成订阅再读取快照：订阅窗口里的切换事件靠 revision 赢过旧快照。
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    setState({ kind: "loading" });
    void listen<TerminalBound>(EVENT_BOUND, (e) => {
      if (active && e.payload.terminal_id === terminalId) applyIdentity(e.payload);
    })
      .then(async (stop) => {
        if (!active) { stop(); return; }
        unlisten = stop;
        const snapshot = await api.terminalAttach(terminalId);
        if (!active) return;
        applyIdentity(snapshot, true);
        setState({ kind: "ready", snapshot, exited: !snapshot.live });
      })
      .catch((error) => {
        if (active) setState({ kind: "error", message: String(error) });
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [terminalId, connectionAttempt, applyIdentity]);

  // 事件不带标题时读取当前会话详情；版本检查同时防住 A→B→A 的迟到请求。
  useEffect(() => {
    if (!identity.sessionId || identity.revision < 0 || identity.title !== null) return;
    const { sessionId, revision } = identity;
    let live = true;
    api.getSessionDetail(sessionId)
      .then((d) => {
        const current = identityRef.current;
        if (!live || current.revision !== revision || current.sessionId !== sessionId || current.title !== null) return;
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
  const exited = state.kind === "ready" && state.exited;
  useEffect(() => {
    if (snapshot === null || containerRef.current === null) return;
    const id = snapshot.terminal_id;
    const container = containerRef.current;

    let entry = liveTerminals.get(id);
    if (!entry) {
      const term = new Terminal({
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        fontSize: 12.5,
        cursorBlink: true,
        scrollback: 5000,
      });
      const stopTheme = observeTerminalTheme(term);
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(container);
      try {
        term.loadAddon(new WebglAddon());
      } catch {
        // WebGL 不可用（驱动、虚拟化）时 xterm 回落到 DOM 渲染，功能不受影响。
      }

      // 复制：有选区时 Cmd/Ctrl+C 是复制，否则交给终端（SIGINT）。
      //
      // 粘贴在 keydown 层截获并压制原生路径，改为后端原生读系统剪贴板一次、
      // 按内容路由：非空文本 → 打字语义交付；图片 → 落盘为 PNG、路径按打字
      // 交付。不走 webview 的 paste 事件——WKWebView 对图片内容会把它派发
      // 两次且 flavor 不一致，事件层怎么防重都会有半份从另一条路漏出去。
      // 中文 IME 的 WKWebView 兼容层。xterm 6 的 input 通路（_inputEvent）在
      // _keyDownSeen 被 keyup 清零后会同步交付 ev.data，而 IME 直提交的事件
      // 顺序 WebKit 不保证：首键只有裸修饰键 keydown（字符键被吞、229 第二键
      // 才出现，探针 A/B 实测），按住修饰键连打时还会对同一次插入派发两次
      // input——单通道、顺序敏感的修法都会漏或多发。这里的交付与顺序无关：
      //   1) 武装窗口由 229 和裸修饰键的 keydown 开启（修饰键不产生可打印
      //      数据，空武装无害）；
      //   2) 武装窗口内的 insertText 在 .xterm 根元素的捕获层统一收口：就地
      //      终结事件（xterm 的 input 通路退场，杜绝它的那份），载荷经差分
      //      扣账交付——窗口内 xterm 经 onData 发过的全部记账，只发余额；
      //      「载荷相同且 textarea 未再变化」视为同一次插入的重复派发，只交
      //      第一份（基线只在 textarea 落上新内容时才推进，跨窗口仍有效）；
      //   3) 组合中间态与窗口外输入一律放行：候选窗归 xterm finalize，emoji
      //      面板等无 keydown 的输入走 xterm 原路径。
      let armed = false;
      let armValue = "";
      let xtermSent = "";
      let composing = false;
      // 重复派发鉴别基线：上一次交付的载荷与其时的 textarea 值。
      let lastPayload = "";
      let lastPayloadValue: string | null = null;
      // 扣账交付：投入的文本先减去 xterm 在本窗口内已经发过的部分——
      // 无论这条文本来自 input 载荷还是 textarea 差分，都恰好交付一次。
      const deliverText = (text: string) => {
        if (!text) return;
        let residue = text;
        if (xtermSent) {
          if (residue.endsWith(xtermSent)) residue = residue.slice(0, residue.length - xtermSent.length);
          if (residue === xtermSent) residue = "";
        }
        if (residue) api.terminalInput(id, residue);
        xtermSent = "";
        armValue = term.textarea?.value ?? armValue;
      };
      const deliver = () => {
        if (!armed || composing || !term.textarea) return;
        const now = term.textarea.value;
        if (now === armValue || !now.startsWith(armValue)) return;
        deliverText(now.slice(armValue.length));
      };
      const armGesture = () => {
        deliver(); // 上一窗口若有未交的残余，先交掉、基线推进
        armed = true;
        armValue = term.textarea?.value ?? "";
        xtermSent = "";
        // 鉴别基线只在 textarea 落上新内容时才推进：同载荷且值未变，跨窗口
        // 仍视为重复派发（重复派发的两份之间可能隔着一个 229 keydown）。
        if (term.textarea?.value !== lastPayloadValue) {
          lastPayload = "";
          lastPayloadValue = term.textarea?.value ?? null;
        }
        setTimeout(deliver, 0);
        setTimeout(deliver, 120);
      };
      term.onData((d) => {
        if (armed) xtermSent += d;
      });
      term.textarea?.addEventListener("compositionstart", () => {
        composing = true;
      });
      term.textarea?.addEventListener("compositionend", () => {
        composing = false;
        setTimeout(deliver, 0);
        setTimeout(deliver, 120);
      });
      // input 的唯一收口（捕获层，先于 xterm 挂在 textarea 上的监听）。
      term.element?.addEventListener(
        "input",
        (e) => {
          const ie = e as InputEvent;
          if (!armed || composing || ie.inputType !== "insertText" || !ie.data) return;
          // 武装窗口内 xterm 的 input 通路退场：_keyDownSeen 已被上一键的
          // keyup 清零时，它会对同一份 ev.data 再交付一次。
          e.stopImmediatePropagation();
          if (ie.data === lastPayload && term.textarea?.value === lastPayloadValue) return;
          lastPayload = ie.data;
          lastPayloadValue = term.textarea?.value ?? null;
          // input 载荷是权威文本：textarea 可能被随时清空，差分读不到，
          // 但载荷永远带着本次插入的内容（首键丢失的正是这一份）。
          deliverText(ie.data);
        },
        true,
      );

      term.attachCustomKeyEventHandler((e) => {
        if (e.type === "keyup") {
          setTimeout(deliver, 0);
          return true;
        }
        if (e.type !== "keydown") return true;
        if (e.keyCode === 229) {
          // IME 接管的键：让 xterm 提前返回（它会对这些键 preventDefault），
          // 并武装差分窗口。
          armGesture();
          return false;
        }
        if (MODIFIER_KEYS.has(e.key)) {
          // 裸修饰键：首键直提交的唯一前置 keydown（探针实测），靠它武装
          // 窗口，随后的 input 载荷才能被 deliverText 接住。放行给 xterm。
          armGesture();
          return true;
        }
        armed = false;
        if ((e.metaKey || e.ctrlKey) && e.key === "c" && term.hasSelection()) {
          void copyToClipboard(term.getSelection());
          return false;
        }
        const pasteKey =
          (e.metaKey && e.key === "v") ||
          (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "v");
        if (pasteKey && !e.repeat) {
          // 返回 false 只是让 xterm 不处理这个键；浏览器对 Cmd+V 的**默认动作**
          // （向 textarea 粘贴并触发 paste 事件）必须显式取消，否则它是一条
          // 独立于本路由的投递通道，键盘路由再正确也会多出一份。
          e.preventDefault();
          void api.readClipboardForTerminal()
            .then((clip) => {
              // 文本也走打字语义（terminalInput）：term.paste 的 bracketed
              // 包裹在部分 TUI 下的重绘是双份渲染的残留层，打字语义是所有
              // TUI 行为的最小公约数；多行文本由 TUI 自己的输入处理消化。
              if (clip.text) {
                api.terminalInput(id, clip.text);
                return;
              }
              if (clip.image_path) {
                // 图片粘贴按 TUI 能力分流（均在真实 PTY 里实测）：
                // claude code 有 ^V 读图绑定——转发控制字节，它自己读剪贴板，
                // 恰好一个 [Image #N]；codex / pi / agy 对 ^V 无反应——交付落盘
                // 路径（打字语义）。agent 取自快照：从 Resume 弹窗直接导航过来
                // 时详情缓存与 props 都可能为空，快照里的 agent 才是权威事实。
                if (snapshot.agent === "claude_code") {
                  api.terminalInput(id, "\x16");
                } else {
                  api.terminalInput(id, clip.image_path);
                }
              }
            })
            .catch(() => {
              // 剪贴板读不到（平台/权限）时无操作，不阻断按键。
            });
          return false;
        }
        return true;
      });

      // 键盘路由是唯一粘贴通道：凡以事件形式到达的粘贴（默认动作残余、Tauri
      // 默认菜单的原生 Paste、右键粘贴）在捕获阶段一律终结，杜绝第二条投递。
      term.element?.addEventListener(
        "paste",
        (e) => {
          e.preventDefault();
          e.stopPropagation();
        },
        true,
      );

      term.write(decodeBase64(snapshot.scrollback));

      // 输入通道随实例常驻：detach 期间没有 UI 焦点自然无输入，无需重建。
      term.onData((data) => {
        const liveNow = liveTerminals.get(id);
        if (!liveNow || liveNow.exited) return;
        api.terminalInput(id, data).catch(() => {
          // 已退出的终端拒绝输入：横幅已说明，不再打扰。
        });
      });

      const unlisteners = [
        listen<string>(`terminal-output://${id}`, (e) => {
          term.write(decodeBase64(e.payload));
        }),
        listen<{ exit_code: number | null }>(`terminal-exit://${id}`, () => {
          const live = liveTerminals.get(id);
          if (live) {
            live.exited = true;
            live.onExit?.();
          }
        }),
      ];
      entry = {
        term,
        fit,
        unlisteners: [stopTheme, ...unlisteners.map((p) => () => {
          void p.then((u) => u()).catch(() => {});
        })],
        exited: !snapshot.live,
        onExit: null,
      };
      liveTerminals.set(id, entry);
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

    const observer = new ResizeObserver(() => {
      try {
        live.fit.fit();
        // 尺寸没变就不打扰 PTY：多余的 resize 会触发 TUI 无谓重绘。
        if (live.term.cols > 0 && live.term.rows > 0) {
          void api.terminalResize(id, live.term.cols, live.term.rows);
        }
      } catch {
        // 容器尺寸不可测（切页瞬间）时跳过这一帧。
      }
    });
    observer.observe(container);
    requestAnimationFrame(() => {
      try {
        live.fit.fit();
      } catch {
        // 首帧尺寸不可测时交给 ResizeObserver。
      }
    });

    return () => {
      live.onExit = null;
      observer.disconnect();
    };
  }, [snapshot]);

  // 未绑定的终端以「新会话」为名；绑定后标题换成会话本名。agent 在
  // attach 回来前用路由种子，避免首帧丢失图标。
  const headerTitle = boundTitle ?? "新会话";
  const headerAgent = snapshot?.agent ?? initialAgent ?? null;

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
            <button
              className="btn ghost icon-button"
              aria-label={reconnecting ? "正在重新连接" : "重新连接"}
              title={boundSessionId ? "重新连接会话" : "绑定会话后可重新连接"}
              disabled={reconnecting || !boundSessionId || state.kind === "loading"}
              onClick={() => void reconnectTerminal()}
            >
              <Icon name="refresh" />
            </button>
            <button
              className="btn ghost icon-button"
              aria-label="会话详情"
              title="会话详情"
              disabled={!boundSessionId}
              onClick={() => {
                if (boundSessionId) navigate({ view: "session", sessionId: boundSessionId });
              }}
            >
              <Icon name="info" />
            </button>
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
            <div className="terminal-host" ref={containerRef} />
          </div>
        </>
      )}
    </div>
  );
}
