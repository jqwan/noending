# Windows 内嵌终端审查与适配

日期：2026-10-14（Asia/Hong_Kong）。工作分支 `main`。已拉取并 rebase 到远程 macOS 适配提交 `122e3eb`，按平台隔离整合，最终检查后提交和推送。开始时唯一已有改动是未跟踪的 `pnpm-workspace.yaml`，其内容保留且不纳入本次提交。未修改数据库格式、摘要服务、会话业务页面或用户 Agent 全局配置。

## 参考和依赖

基准固定为 VS Code 1.141.1，提交 `7baac39ec3ca969dab74ac12c097f6a486e46593`。参考文件下载到 `%TEMP%/noending-vscode-7baac39`，没有使用 main 分支代替该提交。

| 环境 | 实际版本 |
| --- | --- |
| NoEnding 前端 | xterm 6.0.0，Fit 0.11.0，WebGL 0.19.0，React 18.3.1 |
| NoEnding 后端 | Tauri 2.12.1，wry 0.57.0，webview2-com 0.39.1，portable-pty 0.9.0 |
| 实机 | Windows 11，OS build 26300，WebView2/Edge 154；已安装 Pi 1.2.0 |
| VS Code 固定提交 | xterm 6.1.0-beta.304，WebGL 0.20.0-beta.300，node-pty 1.2.0-beta.15；Electron 宿主 |
| 本次新增 | 稳定版 Unicode11 0.9.0、WebLinks 0.12.0 |

NoEnding 原有框架和依赖版本没有升级。新增的两个稳定 addon 在本项目实际 xterm 6.0、WebView2 和构建中验证过，没有照搬 VS Code 的预发布 API。

主要参考：

- [xtermTerminal.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/xterm/xtermTerminal.ts)：配置映射、Unicode addon、WebGL 失效后释放并恢复 DOM、渲染器变化后重新测量。
- [terminalInstance.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/terminalInstance.ts)：焦点、尺寸、输出处理；特别是 `_writeProcessData` 的 write callback 后才 acknowledge。
- [terminal.clipboard.contribution.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminalContrib/clipboard/browser/terminal.clipboard.contribution.ts)：文本通过 `raw.paste`，Windows Ctrl+C 有选区时复制并清区，Ctrl+V/Ctrl+Shift+V，以及右键 copy/paste 与 Shift 覆盖。
- [terminalConfigurationService.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/terminalConfigurationService.ts)、[fontInfo.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/editor/common/config/fontInfo.ts)、[terminalConfiguration.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/common/terminalConfiguration.ts)：Windows Consolas / Courier New / monospace、14px、默认不闪烁的光标。
- [terminalProcess.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/platform/terminal/node/terminalProcess.ts)、[ptyService.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/platform/terminal/node/ptyService.ts)：PTY 背压、缓冲、尺寸、退出和重放。NoEnding 使用原始字节偏移回执及 OS 管道背压，没有移植 node-pty 的 pause/resume 或其字符计数。

也检查了本地安装的 xterm 6.0 source map、Fit/WebGL 源码、portable-pty 的 Windows ConPTY/killer，以及 Pi 的 TUI 渲染代码。

## 已实施

1. **Windows 输入法隔离。** WKWebView 的输入载荷差分、229 拦截和屏幕内 textarea CSS 现在只在 macOS 使用。Windows 保留 xterm 的 Chromium composition/input 路径。终端内按键不会同时冒泡触发应用的 Ctrl+K 或 Alt+方向导航；Tab、Escape、Alt 和无选区 Ctrl+C 仍由 xterm 编码给 Agent。
2. **原生剪贴板和粘贴协议。** Windows 复制和读取都走 arboard；Ctrl+V、Ctrl+Shift+V、Shift+Insert、菜单和普通右键统一使用 xterm.paste。键盘取消浏览器默认动作，菜单事件捕获后仅投递一次。空文本的原生 Ctrl+V 仍发送 `0x16`，供 Agent 自己读取 OS 图片。没有将图片转成路径、文件上传或浏览器图片粘贴。
3. **选择和菜单。** Ctrl+C 有选区复制成功后清区；Ctrl+Shift+C 保留选区。普通右键复制选区或粘贴，鼠标报告开启时保留 TUI 右键。Shift+右键、菜单键、Shift+F10 打开终端菜单；Escape 关闭菜单，F6 离开终端输入。菜单提供持久的屏幕阅读开关，不修改业务页面。
4. **焦点和尺寸。** Windows 挂载、显示恢复、窗口重新激活、字体加载、DPR 变化和渲染器变化统一 refit；隐藏容器不 resize。resize 去重、串行并合并到最新尺寸。后端只在原生 resize 成功后记录尺寸，并限制 ConPTY 可表示的范围。初次回放使用快照尺寸。
5. **字体与 Unicode。** Windows 默认 Consolas、14px、不闪烁光标，启用 Unicode 11；保留 NoEnding 的 5,000 行历史和亮暗主题。没有设置不真实的 Windows build number，也没有移植 Electron/WebKit 专用补丁。
6. **渲染失效恢复。** WebGL 加载失败会释放部分 addon；未恢复的 context loss 会释放 WebGL，恢复 DOM 渲染、刷新原有 buffer，并重新测量尺寸。切页和退出查看仍保留同一 xterm。
7. **输出调度与背压。** 每个 Windows 终端一次 parser write 在途，积压字节合并为最多 64 KiB 的后续批次。write callback 完成后，前端回执已解析的绝对字节位置；后端在 256 KiB 未确认输出时停止继续读，降到 64 KiB 后恢复。每终端独立条件变量；关闭会唤醒等待者。没有用 RAF 等待后台输出，也没有丢弃快速滚动后的队列。
8. **分数缩放下的 viewport 修复。** xterm 6.0 的 viewport 高度会取整，但历史范围使用未取整的 cell 高度。125%/14px 下，31 行 viewport 是 546px，cell 是 17.6px，满历史时原生滚动位置反复被从 88000 压到 87999.6，引发逐行滚动条定时器重置。Windows 适配将范围至少扩展为实际 viewport 高度加可滚动 buffer 行高。该处使用一个有检查和释放逻辑的私有接缝，位于 `terminalViewport.ts`；升级 xterm 时必须重跑探针，不能假定此内部接口稳定。
9. **主题与链接。** Windows 不通过 minimumContrastRatio 改写 Agent 的 RGB。OSC 4/10/11/12 的明确配色在亮暗切换后保留，reset 恢复应用基色；原有颜色查询和 DEC 2031 通知保持。普通 URL 和 OSC 8 仅 Ctrl+点击 HTTP(S)；选区和 TUI 鼠标交互优先。没有 command URI、编辑器引用、工作台或 NoEnding 页面导航。
10. **进程与环境。** Windows 包装使用系统 PowerShell 的明确路径，设置 UTF-8 输入/输出及 truecolor 声明。portable-pty 0.9 的 killer 只结束包装进程，因此 Windows 改用系统 taskkill 结束该包装进程树，再以原生 killer 作失败后备；已退出记录不会再按 PID 杀进程。会话绑定、多个终端、手动重连和退出后查看流程保留。

## 基线与测量

修改前终端测试为 4 文件 / 49 项通过。第一次真实 WebView2 测量在 150% 下，以原有 12.5px、Unicode 6 和 4 KiB write 输入 4,160,000 字节：1,016 次 write，解析完成 110.3ms，P95 callback 延迟 106.7ms。

后续在真实 125% WebView2 中比较；表中均为 parser/render 调度测量，不包含 PTY→Tauri IPC 的端到端延迟，且不是 FPS 指标。短基准有调度和设备负载波动，不能据此声称所有场景更快。

| 配置 | write 次数 | 前台完成 | 同时前台/后台完成 | 最大事件循环间隔 |
| --- | ---: | ---: | ---: | ---: |
| 原有配置构造基线 | 1,016 | 108.1ms | 113.1 / 137.2ms | 25.5ms |
| 14px、Unicode11、合批但未修 viewport | 65 | 348.9ms | 577.6 / 600.0ms | 34.7ms |
| 合批并修 viewport | 65 | 135.1ms | 150.1 / 161.2ms | 33.6ms |

隔离变量实验确认字号带来的实际 canvas/cell 尺寸触发问题；无 renderer 的四种 Unicode/合批组合约 56–60ms。CPU 采样中主要额外工作来自 `_sync → _onDidScroll → _scheduleHide → cancelAndSet` 的 setTimeout/clearTimeout。修复不是固定 FPS 限制，也不是统一加速鼠标输入。

可复现测量工具：`scripts/terminal-webview-probe.mjs`。需要 Node 22+、已安装依赖、真实 Windows/WebView2 和可用 WebGL2。建议在隔离 NoEnding Home 中运行：

```powershell
pnpm dev
# 另一终端；只给测试进程设置环境，不修改系统设置
$env:NOENDING_HOME = Join-Path (Get-Location) '.tmp/terminal-audit/home'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9224'
& .\src-tauri\target\debug\noending.exe
node scripts/terminal-webview-probe.mjs baseline
node scripts/terminal-webview-probe.mjs adapted
```

运行前用 `cargo build --bin noending` 构建当前调试程序。探针的 baseline 构造原有字体/Unicode/write 配置，不能用来验证旧 IME 或旧会话流程。结果显式标记 GPU loss extension 不可用等缺口，需要读输出，不能仅依据退出码宣称设备验证成功。

## 首轮验证结果（远程整合前）

- `pnpm install --frozen-lockfile`：通过。
- `pnpm exec tsc --noEmit`：通过。
- 终端相关 6 文件 / 61 项测试：通过。其中保留并明确标为 macOS 的既有 38 项页面状态机/IME 场景；Windows 协议场景使用真实 xterm parser/encoder。
- `pnpm test`：39 文件 / 291 项通过。
- `pnpm build`：通过；Vite 仍提示主 chunk 超过 500 kB，未将其误报为构建失败。
- `cargo fmt --check`、`cargo check --all-targets`、`cargo test --all-targets`：通过，469 项通过、4 项既有 ignored。ignored 包括真实 Agent 历史、已安装 Agent discovery、需要登录和模型调用的 Codex、以及 Pi + LM Studio 测试，不计为执行通过。
- 全量检查后增强了 Windows cwd 的独立断言，并去掉未经物理触控板测量的手势间隔截止值，保留原有分数距离累计和即时投递；对应真实 PowerShell、终端滚动定向测试和 TS 检查再次通过。
- `git diff --check`：通过。

**真正的 OS/WebView2 实测：**

- 微软拼音 `n → i → Space` 在 Pi 输入框提交一个“你”，中文问号也提交；未提交给模型。候选窗确实出现，但偏位问题见下节。
- Pi 会话切换后“你？”和同一终端 buffer 保留，无重放清空。
- 原生 Shift+F10 打开可访问终端菜单，Escape 关闭后不传入 Agent 的信任选择页。
- 真正的 Ctrl+V + OS 原生剪贴板：仅 1 个 onData，内容精确为一次 bracketed paste（含多行和 emoji）。真正的 Ctrl+C：原生剪贴板内容为预期中文选区，选区清除；测试文本恢复后释放 fixture，无浏览器权限弹窗。
- 真正的 WebGL loss extension：adapted 恢复 DOM，buffer 继续输出 `RECOVERY中文😀`。旧构造基线在相同等待后没有 DOM 回退。这里遵守 addon-webgl 0.19 自身的 3 秒恢复窗口。
- 真正的 OSC 10/11 查询：切换应用亮暗后仍返回 Agent 指定的 `#123456` 和 `#223344`。
- 真实 Windows ConPTY 自动回归：中文输入/UTF-8、子进程读取到实际 101×31 尺寸、包装和子进程树关闭、达到水位暂停并在回执后排空尾部。不是模拟 portable-pty。
- 原生 Alt+F4 清理了无 Agent 的隔离开发实例；运行中的 Agent 关闭由上述 ConPTY 子进程树测试覆盖，尚未在应用退出路径重复实测。

## 首轮问题和设备验证缺口（IME、滚动的后续结果见下节）

| 范围 | 已有依据 | 仍需验证/处理 |
| --- | --- | --- |
| IME | 实际中文候选和组合提交、重复标点自动测试 | Pi 1.2.0 的输入显示在左侧，但 xterm/PTY 光标观测停在末列，候选窗落在右侧。其主屏 renderer 源码会发定位序列，因此尚不能将原因直接归给 Pi；需要对比 ConPTY 原始输出与 VS Code 同 Agent 场景。没有猜测可见光标或发送额外提交文本来掩盖问题。连续长中文、多种标点/输入法尚未完整实测。 |
| 滚动 | 真实 xterm 普通历史、SGR/应用光标协议，小 delta、反转、修饰键、buffer 切换余数清理自动测试；即时合批不保留待发送滚动队列 | 物理精密触控板、快速方向切换及 Pi 全屏内部滚动未实测。全屏 fixture 被 Pi 的目录信任提示挡住；没有改用户 Agent 配置或批准该提示。 |
| 显示 | 150% 初始基线、125% 实际测量，中文/基本 emoji/粗体宽度协议检查 | 100% 实机、完整 125/150% 切换矩阵、全屏窗口、跨显示器实时移动、复杂 ZWJ emoji 尚未完整设备验证。Unicode11 也不等于最新 Unicode/grapheme 支持。 |
| 剪贴板 | 原生文本复制/粘贴、菜单/右键协议和 Ctrl+C 中断编码 | Agent 实际图片粘贴及图文混合剪贴板未实测；图片路径仅验证了原生快捷键字节保留。 |
| 输出/生命周期 | GPU loss、前后台解析测量、每终端背压、真实子进程关闭 | 真正高吞吐 PTY→IPC→UI 的端到端时延/内存采样，大粘贴并发、最小化、系统休眠唤醒、应用退出时多个活跃 Agent 仍待设备验证。输入写入仍是同步 PTY API，不能把输出测量误当作大输入性能结论。 |
| 协议 | bracketed paste、备用屏幕/鼠标报告、颜色查询/reset、Unicode11 自动测试 | 首次晚接入时后端仅保留 256 KiB 原始字节，不是完整 VT 状态序列化；大幅截断后的首次重建仍有状态丢失边界。常规切页使用活的 xterm cache，不走该重建路径。 |
| 可访问性 | 焦点边框、不闪烁默认光标、F6、键盘菜单及屏幕阅读开关 | 未以 Narrator/NVDA 做完整朗读验证；没有可靠的 WebView2 等价 Electron 检测接口。[Windows 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfow) 明确指出 Narrator 不设置 SPI_GETSCREENREADER 标志，因此没有添加一个会漏检 Narrator 的“自动检测”。 |
| 链接 | HTTP(S) Ctrl 激活/拒绝 command 协议的实际 xterm 自动测试 | 普通 URL 跨行选择、OSC 8 的真实浏览器打开及 TUI 鼠标联动仍需设备验证。 |
| 平台 | 新的输入、字体、viewport、背压、进程树和剪贴板适配限定 Windows；旧 macOS 场景测试保留 | macOS 实机和编译未验证，没有运行远程 CI。GUI 重新登录后的 PATH、新安装工具环境与其他已安装 Agent 尚未做完整矩阵验证。 |

Computer-use 技能及其引用的 [guidance.md](C:/Users/jaqwa/.codex/plugins/cache/openai-bundled/computer-use/26.1007.21434/docs/guidance.md) 要求："Do not act on security or privacy permission requests." 对应 [SKILL.md](C:/Users/jaqwa/.codex/plugins/cache/openai-bundled/computer-use/26.1007.21434/skills/computer-use/SKILL.md) 已读取。因此 Pi 目录信任提示没有自动批准，实际全屏场景列为未验证。该限制没有阻止其余代码修复、协议检查和设备测量。

## 用户报告问题的复现与修复

1. **中文输入整体左移**：真实 WebView2 中，微软拼音 `n → i` 令 host.scrollLeft 从 0 增至 6.4px。Chromium 会为了揭示组合 textarea 而滚动 overflow:hidden 容器。修正 Windows host 为 overflow:clip，取消误带入的面板内边距，并取消业务 textarea 的 64px 最小高度影响。macOS 样式保持远程版本。
2. **候选窗在右侧**：Pi 的实际控制台光标在 (0,40)，但系统 ConPTY 隐藏光标时省略最后的定位输出，xterm 留在 (146,17)。VS Code 固定提交默认自带 ConPTY v1.25.260303002；本项目保持系统版本，没有盲目安装该预发布组件。旧 [XtermEngine::PaintCursor](https://github.com/microsoft/terminal/blob/v1.21.10351.0/src/renderer/vt/XtermEngine.cpp#L182) 说明只有可见光标才调用这条绘制路径。Windows helper 只读取已登记 live 终端的实际 viewport 光标，用于组合期间的 textarea/预览定位，不改变 VT buffer，不额外提交输入。GUI 不调用 AttachConsole；helper 用 CONOUT$ 读取原生 buffer，把 JSON 写回原 stdout pipe，释放 console 后退出。5 次实际查询约 17–22ms。前台在安静输出后预取，背景不查询，组合开始再读取最新坐标，缓存尺寸不一致时保留 xterm 原路径。读取失败记录诊断并继续原生输入。
3. **Pi 全屏滚轮变历史按键**：Pi beforeTerminalStart 输出 DECSET 鼠标模式，然后 Node 才 setRawMode。独立真实 ConPTY 探针证明：raw 之后输出 1000/1006 会转发，之前输出会被系统 ConPTY 消耗。Windows shell 包装预先设置 ENABLE_VIRTUAL_TERMINAL_INPUT，保持其余 input mode 位；Agent 仍能自行设置后续模式。Windows 备用屏幕无鼠标报告时阻止 xterm 方向键回退；不能凭空让一个未启用鼠标协议的应用滚动。macOS 回退行为保留。

**实际设备复测**：仅给测试进程设置 PI_CODING_AGENT_DIR，使用隔离 Agent home 和已安装 Pi 1.2.0；没有改用户全局配置，也没有批准目录信任提示。真实 WebView2/微软拼音 `n → i → Space` 只有一个 onData“你”；候选窗在输入框左侧实际光标附近，组合期间 host.scrollLeft=0。Pi 全屏 `/changelog` 本地长内容上、下滚轮各改变 38 行，可见内容随方向变化，SGR wheel 正常，无 CSI A/B 或 SS3 A/B，输入框保持空白。没有模型调用。终端焦点下原生 Alt+F4 清理实例及 Pi；同时修正此前 xterm 吞掉该系统快捷键的路径。

**回归测试**：真实 Windows ConPTY 覆盖鼠标启用顺序、SGR 字节接收、隐藏光标 (2,3)、实际 viewport 尺寸、helper JSON 不进入 Agent 输出。Node.js 与已构建 noending.exe 是声明且缺失会失败的前提。前端覆盖组合单次提交、dispose 后停止查询和 Alt+F4 留给 OS。远程的 macOS keyboard/presentation/cache 和 legacy onBinary 字节输入实现及新增测试均保留。

**最终整合检查**：`pnpm install --frozen-lockfile`、TS 检查通过；终端定向 6 文件 / 69 项，全量前端 39 文件 / 299 项通过；`pnpm build` 通过（原有主 chunk 大小提示仍在）。`cargo fmt --check`、`cargo check --all-targets` 和 `cargo test --all-targets` 通过，470 项通过、4 项既有 ignored。`git diff --check` 通过。

rebase 后再次用真实微软拼音提交一个“你”；组合 textarea 相对 screen 的 left=0、top≈703.6px、height≈17.6px，祖先无横向滚动。Pi 全屏长内容双向滚动再次通过，无方向键投递，Alt+F4 清理了运行中的 Pi 和隔离实例。重新加载前端后重新附着同一个 PTY 的输出也恢复正常。真实 WebGL context loss 仍恢复 DOM，保留 Agent 颜色查询回复；4,160,000 字节合批为 65 次 write，前台/后台完成约 118/139ms，最大事件循环间隔约 29/25ms。这仍是 parser/render 调度测量，不能代替 PTY→IPC→UI 端到端性能。

光标诊断可在干净的开发服务器和隔离 WebView2 中运行 `node scripts/terminal-cursor-probe.mjs`；先聚焦 live Agent 终端，再触发真实输入法组合。探针只读原生/xterm 光标、输入法锚点、耗时和祖先滚动位置，不发送 Agent 输入。缺少真实 WebView2、live 焦点或原生光标会失败。

物理精密触控板、100% 缩放、休眠唤醒、真实图片粘贴、屏幕阅读器及 macOS 实机仍未验证，不能用自动测试替代。

这次适配有可验证的行为修复，仍不等同于 VS Code 完整默认体验；WebView2、ConPTY/portable-pty 和 xterm 版本差异仍然存在。
