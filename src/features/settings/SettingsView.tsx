import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { Modal, openPath, submitsOnEnter } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { WorkspaceSettings } from "../../types";
import type { Route, SettingsSection, LegacySettingsSection } from "../../app/routes";

/** Settings：设置页合并为统一平铺视图，聚焦于全局系统设置。 */
/** Settings：设置页合并为统一平铺视图，聚焦于主题外观与本地存储数据。 */
export default function SettingsView({ section: _section, navigate: _navigate }: {
  section?: SettingsSection | LegacySettingsSection;
  navigate?: (r: Route) => void;
}) {
  return (
    <div className="main settings-page">
      <PageHeader
        title="设置"
        sub="配置主题外观、Context 诊断与本地存储数据。"
      />
      <div className="settings-pane" style={{ marginTop: 18 }}>
        <AppearanceSettings />
        <ContextDiagnosticsSettings />
        <WorkspaceStorageSettings />
      </div>
    </div>
  );
}

/** Appearance：主题设置（tokens 支持暗色）；Density 暂缓。 */
type Theme = "system" | "light" | "dark";
const THEME_LABELS: Record<Theme, string> = { system: "跟随系统", light: "浅色", dark: "深色" };
function AppearanceSettings() {
  const [theme, setTheme] = useState<Theme>(() => {
    const saved = localStorage.getItem("noending.theme");
    return saved === "light" || saved === "dark" ? saved : "system";
  });

  const apply = (t: Theme) => {
    setTheme(t);
    if (t === "system") {
      localStorage.removeItem("noending.theme");
      delete document.documentElement.dataset.theme;
    } else {
      localStorage.setItem("noending.theme", t);
      document.documentElement.dataset.theme = t;
    }
  };

  return (
    <section>
      <h3 style={{ marginTop: 0 }}>主题</h3>
      <div className="theme-options" role="group" aria-label="主题">
        {(Object.keys(THEME_LABELS) as Theme[]).map((t) => (
          <button key={t} className={theme === t ? "on" : ""} aria-pressed={theme === t} onClick={() => apply(t)}>
            <span className={`theme-preview theme-preview-${t}`} aria-hidden="true"><span /><span /></span>
            {THEME_LABELS[t]}
          </button>
        ))}
      </div>
      <p className="muted small" style={{ marginBottom: 0 }}>
        跟随系统时自动切换浅色 / 深色。
      </p>
    </section>
  );
}

function ContextDiagnosticsSettings() {
  const openLogs = async () => {
    try {
      await api.openContextExtractionLogs();
      showToast("已打开 Context 更新日志目录");
    } catch (error) {
      console.error(error);
      showToast("无法打开 Context 更新日志目录");
    }
  };

  return (
    <section>
      <h3>Context 更新诊断</h3>
      <div className="row-line">
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="settings-row-label">本地运行日志</div>
          <div className="settings-row-hint">
            记录更新时间、目标、Agent 和失败阶段；不记录提示词、模型输出或终端错误文本，保留 14 天。
          </div>
        </div>
        <button className="btn small" onClick={openLogs}>打开日志目录</button>
      </div>
    </section>
  );
}

/* NoEnding Home：说的必须是当前真正在写的那一份。`get_workspace_settings` 读的是
 * 启动时解析并 manage 进应用的 Home（不重新解析）；改位置不切库，只写 bootstrap
 * 指针的 `pending_home`，下次启动打开数据库前才移动——所以 `restart_required` 与
 * `pending_home` 必须原样显示，不能吞掉。 */

/** `home_source` 的中文说明。 */
function homeSourceLabel(source: string | null | undefined): string {
  switch (source) {
    case "explicit_env":
      return "由环境变量 NOENDING_HOME 指定";
    case "bootstrap":
      return "由 NoEnding 的启动指针记录";
    case "default_home":
      return "系统用户目录下的默认位置";
    case null:
    case undefined:
    case "":
      return "未知来源";
    default:
      return source;
  }
}

function WorkspaceStorageSettings() {
  const [settings, setSettings] = useState<WorkspaceSettings | null>(null);
  const [failed, setFailed] = useState(false);
  const [editing, setEditing] = useState(false);

  const load = useCallback(() => {
    api.getWorkspaceSettings()
      .then((s) => { setSettings(s); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); });
  }, []);
  useEffect(load, [load]);

  if (!settings) {
    return (
      <section>
        <h3>NoEnding Home</h3>
        {failed ? (
          <>
            <div className="l1-none">
              读取数据位置失败，请重试。
            </div>
            <div className="invite">
              <button className="btn small" onClick={load}>重试</button>
            </div>
          </>
        ) : (
          <div className="muted small">读取中…</div>
        )}
      </section>
    );
  }

  /* restart_required 与 pending_home 是两个信号，各自都要露出来：任一字段单独出现
     都意味着有件事还没落地，不能被另一个字段的空值吞掉。 */
  const pending = settings.pending_home;
  const relocationPending = settings.restart_required || pending !== null;
  const envPinned = settings.home_source === "explicit_env";

  return (
    <section>
      <h3>NoEnding Home</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        存放数据库、运行文件和日志；默认工作目录为 <span className="mono">workspace/</span>。
      </p>

      <PathRow
        label="NoEnding Home"
        value={settings.noending_home}
        hint={homeSourceLabel(settings.home_source)}
        action={
          <div className="row" style={{ gap: 8 }}>
            {relocationPending && <span className="badge warn">待重启生效</span>}
            <button className="btn small" onClick={() => setEditing(true)}>更改位置…</button>
          </div>
        }
      />
      <PathRow label="默认工作目录" value={settings.default_workspace} />
      <PathRow
        label="数据库路径"
        value={settings.db_path}
        hint={relocationPending ? "当前仍在写入的位置；迁移在下次启动时进行。" : undefined}
      />
      <div className="row" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn small ghost" onClick={load}>重新读取</button>
        {failed && (
          <span className="badge warn" title="刚才这次重新读取失败了；上面显示的是上一次成功读到的内容。">
            重新读取失败，显示的是上一次的结果
          </span>
        )}
      </div>

      {envPinned && (
        <WarnCallout title="环境变量优先级更高">
          <div>
            当前 Home 由环境变量 <span className="mono">NOENDING_HOME</span> 决定，它的优先级高于
            NoEnding 自己记录的指针（<span className="mono">NOENDING_HOME → 启动指针 → 默认位置</span>）。
          </div>
          <div>
            在取消这个变量之前，「更改位置」写下的新路径在下次启动也不会生效。
          </div>
        </WarnCallout>
      )}

      {relocationPending && (
        <WarnCallout title="迁移已安排，重启后生效">
          <div>
            {pending ? (
              <>
                下次启动时 NoEnding 会把数据搬到{" "}
                <span
                  role="button"
                  tabIndex={0}
                  className="mono path-link"
                  style={{ wordBreak: "break-all" }}
                  title={`${pending} · 点击在文件管理器中打开`}
                  onClick={() => void openPath(pending)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      void openPath(pending);
                    }
                  }}
                >
                  {pending}
                </span>
                ，搬完才打开数据库。
              </>
            ) : (
              <>
                位置变更已经登记，但没有读到目标路径（点「重新读取」再看一次）。
                在此之前搬迁不会发生。
              </>
            )}
            当前进程仍在写入上面那个数据库路径。
          </div>
          <div>
            会搬的只有 NoEnding 自己拥有的 <span className="mono">data/</span>、
            <span className="mono">runtime/</span>、<span className="mono">logs/</span>
            ；跨磁盘时会复制并保留原件。
          </div>
          <div>
            <strong>旧 Home 的 <span className="mono">workspace/</span> 里的文件不会被移动。</strong>
            它们留在原处，那个目录之后作为一个普通工作目录被识别（它仍然带着自己的项目与历史）。
          </div>
          <div>
            新 Home 下的 <span className="mono">workspace/</span> 会成为新的默认工作目录。
          </div>
        </WarnCallout>
      )}

      {editing && (
        <ChangeHomeModal
          current={settings}
          onClose={() => setEditing(false)}
          onSaved={(next) => { setSettings(next); setEditing(false); }}
        />
      )}
    </section>
  );
}

/** 一行「标签 + 完整路径」；路径一律整段可换行显示，绝不省略成看不见的样子。 */
function PathRow({ label, value, hint, action }: {
  label: string;
  value: string | null | undefined;
  hint?: string;
  action?: React.ReactNode;
}) {
  const hasValue = value && value.trim() !== "";
  return (
    <div className="row-line">
      <div style={{ minWidth: 0, flex: 1 }}>
        <div className="settings-row-label">{label}</div>
        <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
          {hasValue ? (
            <span
              role="button"
              tabIndex={0}
              className="path-link"
              title={`${value} · 点击在文件管理器中打开`}
              onClick={() => void openPath(value!)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  void openPath(value!);
                }
              }}
            >
              {value}
            </span>
          ) : (
            "—"
          )}
        </div>
        {hint && <div className="settings-row-hint" style={{ wordBreak: "break-word" }}>{hint}</div>}
      </div>
      {action && <div style={{ flex: "none" }}>{action}</div>}
    </div>
  );
}

/** 成段的警告块：仓库里还没有公共 callout 组件，先用现有 tokens 拼一个。 */
function WarnCallout({ title, children }: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{
      marginTop: 12,
      padding: "10px 12px",
      borderRadius: "var(--radius-sm)",
      border: "1px solid var(--warning)",
      background: "var(--warning-wash)",
      color: "var(--text-primary)",
      fontSize: 12.5,
      lineHeight: 1.7,
      display: "grid",
      gap: 4,
    }}>
      <div style={{ color: "var(--warning)", fontWeight: 600 }}>{title}</div>
      {children}
    </div>
  );
}

/**
 * 更改 NoEnding Home：只登记下一次启动要搬去的地方（同进程换库会留下两份分叉写入）。
 * 说清两句：只搬应用拥有的 data/ runtime/ logs/，旧 Home 的 workspace/ 用户文件不动。
 */
function ChangeHomeModal({ current, onClose, onSaved }: {
  current: WorkspaceSettings;
  onClose: () => void;
  onSaved: (next: WorkspaceSettings) => void;
}) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const trimmed = path.trim();
  const sameAsCurrent = trimmed !== "" && trimmed === current.noending_home.trim();
  const canSubmit = trimmed !== "" && !sameAsCurrent && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError("");
    try {
      onSaved(await api.setNoendingHome(trimmed));
      showToast("迁移已安排：重启 NoEnding 后生效。");
    } catch (e) {
      console.error(e);
      // 后端会拒绝空路径、相对路径与「和当前位置相同」，理由原样给用户，不静默回退。
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="更改 NoEnding Home" onClose={onClose}>
      <label className="field">
        <span>新的位置</span>
        <input
          type="text"
          className="mono"
          value={path}
          placeholder={current.noending_home}
          autoFocus
          onChange={(e) => { setPath(e.target.value); setError(""); }}
          onKeyDown={(e) => { if (submitsOnEnter(e)) void submit(); }}
          disabled={busy}
        />
      </label>
      <div className="settings-row-hint" style={{ marginBottom: 10, wordBreak: "break-word" }}>
        当前：<span className="mono" style={{ wordBreak: "break-all" }}>{current.noending_home}</span>
        （{homeSourceLabel(current.home_source)}）。支持 <span className="mono">~</span> 开头的写法；
        相对路径会被拒绝，请给一个绝对路径。
      </div>

      <WarnCallout title="这会怎样发生">
        <div>
          现在这一步只是<strong>登记</strong>：重启 NoEnding 后，它会在打开数据库之前完成搬迁，
          搬迁成功之前不会切过去。所以这一项<strong>要重启才生效</strong>。
        </div>
        <div>
          搬走的是 <span className="mono">data/</span>（数据库）、<span className="mono">runtime/</span>、
          <span className="mono">logs/</span>。
        </div>
        <div>
          如果新位置上<strong>已经有</strong> NoEnding 数据，它不会被覆盖：下次启动会直接打开那里的数据库，
          而现在这份会原样留在旧 Home 里（没有丢，只是不再被这个应用读取）。
        </div>
        <div>
          <strong>旧 Home 的 <span className="mono">workspace/</span> 里的文件不会被移动</strong>
          ：它们留在原来的位置，那个目录此后只是一个普通的工作目录。要保留它们，就自己把它们搬到
          新 Home 的 <span className="mono">workspace/</span> 下面，或者继续用原路径打开。
        </div>
        <div>
          新的默认工作目录会是新 Home 下的 <span className="mono">workspace/</span>，
          已有会话与任务的历史不会因为这次搬迁被改写。
        </div>
        {current.home_source === "explicit_env" && (
          <div>
            注意：<span className="mono">NOENDING_HOME</span> 环境变量仍然优先。只要它还在，
            下次启动依旧会打开它指定的那个 Home。
          </div>
        )}
      </WarnCallout>

      {sameAsCurrent && (
        <div className="muted small" style={{ marginTop: 10 }}>
          这就是当前的位置，不需要迁移。
        </div>
      )}
      {error && (
        <div className="badge warn" style={{
          marginTop: 10, display: "block", padding: "8px 10px", lineHeight: 1.6,
          wordBreak: "break-word",
        }}>
          {error}
        </div>
      )}

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={!canSubmit} onClick={submit}>
          {busy ? "登记中…" : "登记并在下次启动迁移"}
        </button>
      </div>
    </Modal>
  );
}
