import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import PageHeader from "../../layout/PageHeader";
import { copyToClipboard } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { api } from "../../api";
import SessionSubpageTabs from "./SessionSubpageTabs";
import SessionHeaderActions from "./SessionHeaderActions";
import { sessionDisplayTitle } from "./SessionTable";
import { sessionDetailCache } from "./SessionDetailView";
import type { Agent, SessionDetail, TerminalSnapshot } from "../../types";
import type { Route } from "../../app/routes";

/**
 * 会话的第三个子页：内嵌 TUI 终端。PTY 与 scrollback 都归后端
 * （terminal registry），本组件只是 attach/detach 客户端——切走子页（remount）
 * 进程照跑，切回来重放 scrollback 快照。
 *
 * 进入即用：没有内嵌终端时自动直启一次内嵌 Resume（launch_embedded_resume，
 * 不经过继续会话弹窗），失败给出原因和重试。已退出的终端回放只读，横幅里的
 * 「再次启动」直接再开一个新终端。
 *
 * 已知的产品边界：关闭 NoEnding 会结束后端里的内嵌 Agent（后端 RunEvent::Exit
 * 统一收割）；要挂后台的会话走外部终端。
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
  | { kind: "launching" }
  | { kind: "error"; message: string }
  | { kind: "ready"; snapshot: TerminalSnapshot; exited: boolean };

/**
 * 常驻 xterm 实例，按 terminal_id 键控（VS Code 同款）。切走子页只让 DOM
 * 节点脱离文档，Terminal / scrollback / 尺寸 / 监听全部存活，detach 期间的
 * PTY 输出持续写入同一个实例；切回只需把节点搬回新容器——不做 scrollback
 * 重放。重放是布局错乱的根源：字节流里的光标定位/清屏序列绑定产生时的终端
 * 尺寸，重放视口稍有出入（挂载瞬间的 fit 抖动、resize 竞态）就渲染错乱，
 * 还会把错误尺寸发给 PTY 触发 TUI 重绘错位、滑不动。实例常驻后整类问题
 * 不存在。注册表随会话数量有界；「再次启动」会销毁旧实例。
 */
type LiveTerminal = {
  term: Terminal;
  fit: FitAddon;
  unlisteners: (() => void)[];
  exited: boolean;
  /** 组件挂载期间注册；exit 事件经它把横幅翻转到当前挂载的视图上。 */
  onExit: (() => void) | null;
};
const liveTerminals = new Map<string, LiveTerminal>();

/** 测试钩子：模块级注册表会跨用例存活。 */
export function resetLiveTerminalsForTests(): void {
  for (const entry of liveTerminals.values()) {
    entry.unlisteners.forEach((u) => u());
    entry.term.dispose();
  }
  liveTerminals.clear();
}

/** 与 Resume 门槛同一套语义：回收站 / 源不可用都不能再开终端（后端同样拒绝）。 */
function terminalGateOf(detail: SessionDetail | undefined): string | null {
  if (!detail) return null;
  if (detail.can_resume) return null;
  return detail.source_status === "missing"
    ? "源会话已不存在，无法继续"
    : "无法确认源会话状态，暂时不能继续";
}

export default function SessionTerminalView({
  sessionId,
  initialTitle,
  initialAgent,
  navigate,
}: {
  sessionId: string;
  initialTitle?: string;
  initialAgent?: Agent;
  navigate: (r: Route) => void;
}) {
  const cached = sessionDetailCache.get(sessionId);
  const agent = cached?.session.agent ?? initialAgent ?? null;
  const displayTitle = cached
    ? sessionDisplayTitle(cached.session.title)
    : sessionDisplayTitle(initialTitle);

  const [state, setState] = useState<TerminalState>({ kind: "loading" });
  const containerRef = useRef<HTMLDivElement | null>(null);
  // 头部动作簇（回收/同步/继续）与终端段的能力判别需要会话事实（回收站态、
  // source_kind）：详情是纯读取，进来读一次并写缓存，动作后刷新。
  const [detail, setDetail] = useState<SessionDetail | null>(
    () => sessionDetailCache.get(sessionId) ?? null,
  );
  const refreshDetail = useCallback(() => {
    api.getSessionDetail(sessionId)
      .then((d) => {
        sessionDetailCache.set(sessionId, d);
        setDetail(d);
      })
      .catch(() => {
        // 详情读不到时头部按 props 兜底；终端本身不受影响。
      });
  }, [sessionId]);
  useEffect(() => {
    refreshDetail();
  }, [refreshDetail]);

  /**
   * 进入即用：查已有终端 → 有则 attach；没有则自动直启一次内嵌 Resume 再
   * attach。门槛（回收站 / 源不可用）优先于启动——与 Resume 完全同源。
   */
  const bootstrap = useCallback(() => {
    setState({ kind: "loading" });
    const attachOrCreate = async (): Promise<TerminalSnapshot> => {
      const summary = await api.terminalForSession(sessionId);
      if (summary) return api.terminalAttach(summary.terminal_id);
      const gate = terminalGateOf(sessionDetailCache.get(sessionId));
      if (gate) throw new Error(gate);
      setState({ kind: "launching" });
      await api.launchEmbeddedResume(sessionId);
      const created = await api.terminalForSession(sessionId);
      if (!created) throw new Error("启动已完成，但找不到内嵌终端记录");
      return api.terminalAttach(created.terminal_id);
    };
    attachOrCreate()
      .then((snapshot) => {
        setState({ kind: "ready", snapshot, exited: !snapshot.live });
      })
      .catch((error) => {
        setState({ kind: "error", message: String(error) });
      });
  }, [sessionId]);

  useEffect(() => {
    bootstrap();
  }, [bootstrap]);

  /** 退出横幅的「再次启动」：跳过已退出终端的 attach，强制新开一个内嵌终端。 */
  const relaunch = useCallback(() => {
    setState({ kind: "launching" });
    // 旧实例就地销毁：它的 Agent 已退出，「再次启动」开的是新 PTY、新实例。
    if (state.kind === "ready") {
      const old = liveTerminals.get(state.snapshot.terminal_id);
      if (old) {
        old.unlisteners.forEach((u) => u());
        old.term.dispose();
        liveTerminals.delete(state.snapshot.terminal_id);
      }
    }
    api.launchEmbeddedResume(sessionId)
      .then(() => api.terminalForSession(sessionId))
      .then((created) => {
        if (!created) throw new Error("启动已完成，但找不到内嵌终端记录");
        return api.terminalAttach(created.terminal_id);
      })
      .then((snapshot) => {
        setState({ kind: "ready", snapshot, exited: !snapshot.live });
      })
      .catch((error) => {
        setState({ kind: "error", message: String(error) });
      });
  }, [sessionId, state]);

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
        unlisteners: unlisteners.map((p) => () => {
          void p.then((u) => u()).catch(() => {});
        }),
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

  return (
    <div className="main fill">
      <PageHeader
        title={
          agent ? (
            <span className="session-title-with-icon" title={displayTitle}>
              <AgentIcon agent={agent} size={18} />
              <span className="session-title-text">{displayTitle}</span>
            </span>
          ) : (
            displayTitle
          )
        }
        actions={(
          <span className="row" style={{ gap: 8 }}>
            <SessionSubpageTabs
              sessionId={sessionId}
              entry="terminal"
              agent={agent}
              sourceKind={detail?.session.source_kind}
              terminalGate={terminalGateOf(cached)}
              navigate={navigate}
            />
            {agent && (
              <SessionHeaderActions
                sessionId={sessionId}
                agent={agent}
                sourceKind={detail?.session.source_kind}
                title={displayTitle}
                trashed={!!detail?.session.trashed_at}
                onChanged={refreshDetail}
              />
            )}
          </span>
        )}
      />

      {state.kind === "loading" && <div className="empty">正在连接内嵌终端…</div>}
      {state.kind === "launching" && (
        <div className="empty">正在启动内嵌终端：Agent 将恢复此会话…</div>
      )}
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
          {exited && (
            <div className="terminal-exit-banner" role="status">
              这个终端里的 Agent 已退出
              {snapshot.exit_code !== null && <>（退出码 {snapshot.exit_code}）</>}
              。对话记录会随摄入出现在概览与对话页。
              <button className="btn small" style={{ marginLeft: 10 }} onClick={relaunch}>
                再次启动
              </button>
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
