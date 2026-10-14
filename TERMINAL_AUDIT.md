# Windows 内嵌终端适配

基准为 VS Code 1.141.1（`7baac39ec3ca969dab74ac12c097f6a486e46593`）。NoEnding 保持 Tauri / WebView2 / portable-pty 架构；参考终端协议和生命周期处理，不移植 Electron 或工作台能力。

## 参考实现

- [terminalInstance.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/terminalInstance.ts)：xterm write callback 完成后才确认输出。
- [terminal.clipboard.contribution.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminalContrib/clipboard/browser/terminal.clipboard.contribution.ts)：原生剪贴板文本经 xterm.paste 投递，保留 bracketed paste。
- [terminalContextMenu.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/terminalContextMenu.ts)：菜单隐藏与焦点恢复。
- [xtermTerminal.ts](https://github.com/microsoft/vscode/blob/7baac39ec3ca969dab74ac12c097f6a486e46593/src/vs/workbench/contrib/terminal/browser/xterm/xtermTerminal.ts)：字体、Unicode、WebGL 回退；未新增全平台滚轮策略，备用屏幕无鼠标报告时仍保留 xterm 原生方向键回退。

## 实现边界

- macOS 的 WKWebView IME 修复保持独立；Windows 使用 xterm 的 Chromium 输入路径。
- Windows 默认 Consolas / Courier New / monospace、14px、光标不闪烁；启用 Unicode11 0.9。xterm 6.0、Fit 0.11、WebGL 0.19、WebLinks 0.12 保持原版本。
- 文本粘贴逐次读取剪贴板，按用户操作顺序投递。键盘取消浏览器默认粘贴，菜单事件经同一队列，防止重复投递。图片仍由 Agent 通过原生 Ctrl+V 自行读取。
- 右键复制或粘贴；鼠标协议开启时保留 Agent 右键。Shift+右键、菜单键或 Shift+F10 打开菜单。菜单随终端页面关闭或焦点离开而关闭；缓存终端仍接收输出。
- 字体加载、DPR、窗口恢复和渲染器变化触发 Windows refit，隐藏容器不 resize。后端只在原生 resize 成功后记录尺寸。
- WebGL 管理复用各平台共有实现：失效后释放 addon、恢复 DOM、刷新原 buffer 并重新测量。
- Windows 每终端独立 parser 队列；积压的后续块合批约 64 KiB。xterm 解析后确认字节位置；未确认输出达到 256 KiB 时暂停读取，降至 64 KiB 后恢复。
- 只读快照和后台原生会话身份查询不获取输出流控资格。前端订阅持有独立 consumer ID；首次 attach 未完成就离开时，等待请求结束后释放。迟到的旧 ACK / release 不改变新接收者的信用，取消会唤醒读取线程。ACK 失败退避重试；常规切页保留缓存实例及其接收资格。
- xterm 6.0 在分数缩放下的 viewport 修复使用带检查和释放的私有接缝（terminalViewport.ts）。升级 xterm 后必须复测，不假定内部接口稳定。
- 明确的 Agent OSC 配色跨亮暗切换保留，reset 恢复应用基色。HTTP(S) 链接使用原生 Ctrl+点击；不解释编辑器引用或 command URI。
- PowerShell 包装明确使用系统路径，设置 UTF-8、truecolor 和初始 VT input mode。Pi 在 Node raw mode 之前输出的鼠标协议因此能经过系统 ConPTY 转发；不再为此禁用所有备用屏幕滚轮回退。
- Windows 关闭包装进程树使用系统 taskkill /T /F，失败后使用 portable-pty 原生 killer；已退出记录不重复按 PID 杀进程。
- 系统 ConPTY 可能省略隐藏光标的 VT 定位输出。辅助进程只读取实际控制台光标用于输入法定位；仅在前台输入法组合期间查询，不再因普通 Agent 输出预取。尺寸不符、查询失败或组合结束时恢复 xterm 定位，不修改 VT buffer 或提交内容。

## 验证与复测

2026-10-14 的原 Windows 提交 `7f4546f` 记录了 Windows 11 build 26300、WebView2 154、Pi 1.2.0 的实机验证：微软拼音、原生复制粘贴、Pi 全屏滚动、WebGL 失效、OSC 配色、ConPTY UTF-8 / resize / 进程树关闭。后续修复不能直接沿用这些设备验证结论，仍需 Windows 实机复测。

自动回归覆盖 xterm 真实 parser / encoder、连续及乱序完成的剪贴板读取、菜单导航生命周期、输入法查询范围、取消 attach、ACK 重试、流控释放及迟到消息。Windows ConPTY 测试要求 Node.js、系统 PowerShell 和已构建的 GUI 程序；缺少前提会失败，不静默跳过。

可复现探针：

```powershell
pnpm dev
# 另一终端：使用隔离应用数据目录；不要修改系统环境。
$env:NOENDING_HOME = Join-Path (Get-Location) '.tmp/terminal-audit/home'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9224'
cargo build --manifest-path src-tauri/Cargo.toml --bin noending
& .\src-tauri\target\debug\noending.exe
node scripts/terminal-webview-probe.mjs baseline
node scripts/terminal-webview-probe.mjs adapted
# 聚焦运行中的 Agent 终端后，检查原生光标 / 输入法锚点。
node scripts/terminal-cursor-probe.mjs
```

WebView2 探针要求 Node 22+，测量的是 parser / render 调度，不是 FPS 或 PTY → IPC → UI 端到端性能。原记录的旧配置约 108ms、适配后约 118–135ms（4,160,000 字节），不能据此声称整体更快。

物理触控板、100% 缩放、跨屏缩放、休眠恢复、真实图片粘贴、屏幕阅读器及完整 Agent 矩阵仍需设备验证。首次接入仅能重放最多 256 KiB 原始字节，并非完整 VT 状态快照；普通切页复用活的 xterm 实例。
