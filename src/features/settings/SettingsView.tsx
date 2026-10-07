import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { Modal, copyToClipboard, openPath, submitsOnEnter } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
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

/* 数据存储目录（原 NoEnding Home）：
 * `get_workspace_settings` 读取的是启动时解析并管理进应用的存储根目录（NOENDING_HOME）。
 * 修改位置不直接切库，只写启动指针中的 pending_home，在下次启动打开数据库前执行数据搬迁。
 * 因此 restart_required 与 pending_home 必须清晰显示。 */

/** `home_source` 的中文说明。 */
function homeSourceLabel(source: string | null | undefined): string {
  switch (source) {
    case "explicit_env":
      return "环境变量 NOENDING_HOME";
    case "bootstrap":
      return "应用配置记录";
    case "default_home":
      return "系统默认路径";
    case null:
    case undefined:
    case "":
      return "未知来源";
    default:
      return source;
  }
}

/** 可交互路径链接：点击在系统文件管理器中打开。 */
function ClickablePath({ path, title }: { path: string; title?: string }) {
  return (
    <span
      role="button"
      tabIndex={0}
      className="path-link mono"
      style={{ wordBreak: "break-all" }}
      title={title ?? `${path} · 点击在文件管理器中打开`}
      onClick={() => void openPath(path)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          void openPath(path);
        }
      }}
    >
      {path}
    </span>
  );
}

/** 路径复制小图标按钮。 */
function CopyButton({ value, title = "复制路径" }: { value: string; title?: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    const ok = await copyToClipboard(value);
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
      showToast("已复制到剪贴板");
    }
  };
  return (
    <button
      type="button"
      className={`btn ghost icon-button small session-field-copy-btn${copied ? " copied" : ""}`}
      title={copied ? "已复制" : title}
      aria-label={copied ? "已复制" : title}
      onClick={onCopy}
    >
      <Icon name={copied ? "check" : "copy"} />
    </button>
  );
}

function WorkspaceStorageSettings() {
  const [settings, setSettings] = useState<WorkspaceSettings | null>(null);
  const [failed, setFailed] = useState(false);
  const [editing, setEditing] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(() => {
    setRefreshing(true);
    api.getWorkspaceSettings()
      .then((s) => { setSettings(s); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); })
      .finally(() => setRefreshing(false));
  }, []);
  useEffect(load, [load]);

  if (!settings) {
    return (
      <section>
        <div className="settings-section-header">
          <div>
            <h3>数据存储目录</h3>
            <p className="muted small" style={{ margin: "4px 0 0" }}>
              管理应用数据库、运行日志与默认工作区的存储路径。
            </p>
          </div>
        </div>
        {failed ? (
          <>
            <div className="l1-none">
              读取数据存储配置失败，请检查文件系统权限或稍后重试。
            </div>
            <div className="invite" style={{ marginTop: 10 }}>
              <button className="btn small" onClick={load}>重试</button>
            </div>
          </>
        ) : (
          <div className="muted small" style={{ padding: "16px 0" }}>正在读取存储配置…</div>
        )}
      </section>
    );
  }

  const pending = settings.pending_home;
  const relocationPending = settings.restart_required || pending !== null;
  const envPinned = settings.home_source === "explicit_env";

  return (
    <section>
      <div className="settings-section-header">
        <div>
          <h3>数据存储目录</h3>
          <p className="muted small" style={{ margin: "4px 0 0" }}>
            管理应用数据库、运行日志与默认工作区的存储路径。
          </p>
        </div>
        <button
          className="btn small ghost icon-button"
          title="重新读取存储配置"
          aria-label="重新读取存储配置"
          onClick={load}
          disabled={refreshing}
        >
          <Icon name="refresh" />
        </button>
      </div>

      <div className="settings-storage-card">
        <div className="settings-storage-card-header">
          <div className="settings-storage-card-title">
            <Icon name="folder" />
            <span>存储根目录</span>
            <span className="badge" title="系统环境变量标识：NOENDING_HOME">NOENDING_HOME</span>
            <span className="badge" title="路径来源">{homeSourceLabel(settings.home_source)}</span>
            {relocationPending && <span className="badge warn">待重启生效</span>}
          </div>
          <button className="btn small primary" onClick={() => setEditing(true)}>更改位置…</button>
        </div>

        <div className="settings-storage-path-box">
          <div className="settings-storage-path-text">
            <ClickablePath path={settings.noending_home} />
          </div>
          <CopyButton value={settings.noending_home} title="复制存储根目录路径" />
        </div>

        <div className="muted small" style={{ margin: 0 }}>
          存放应用元数据、运行缓存与日志；未指定工作区的新建会话默认以其子目录 <span className="mono">workspace/</span> 作为工作空间。
        </div>
      </div>

      <div className="settings-storage-subpaths">
        <div className="settings-storage-subpath-item">
          <div className="settings-storage-subpath-header">
            <div className="settings-storage-subpath-title">
              <Icon name="folder" />
              <span>默认工作目录</span>
            </div>
            <CopyButton value={settings.default_workspace} title="复制默认工作目录路径" />
          </div>
          <div className="settings-storage-subpath-path">
            <ClickablePath path={settings.default_workspace} />
          </div>
          <div className="muted small" style={{ margin: 0 }}>
            未绑定任务或项目的独立会话默认在此目录下启动。
          </div>
        </div>

        <div className="settings-storage-subpath-item">
          <div className="settings-storage-subpath-header">
            <div className="settings-storage-subpath-title">
              <Icon name="database" />
              <span>SQLite 数据库</span>
            </div>
            <CopyButton value={settings.db_path} title="复制数据库路径" />
          </div>
          <div className="settings-storage-subpath-path">
            <ClickablePath path={settings.db_path} />
          </div>
          <div className="muted small" style={{ margin: 0 }}>
            {relocationPending ? "当前仍在写入此位置；搬迁将在下次重启时执行。" : "本地数据库文件，记录任务图、会话流与配置元数据。"}
          </div>
        </div>
      </div>

      {failed && (
        <div className="badge warn" style={{ marginTop: 12, padding: "8px 12px", display: "inline-block" }}>
          重新读取失败，显示的是上一次成功读取的结果。
        </div>
      )}

      {envPinned && (
        <WarnCallout title="环境变量优先级更高">
          <div>
            当前存储目录由环境变量 <span className="mono">NOENDING_HOME</span> 决定，其优先级高于应用内部记录的配置。
          </div>
          <div>
            在取消该环境变量之前，「更改位置」设置的新路径在重启后不会生效。
          </div>
        </WarnCallout>
      )}

      {relocationPending && (
        <WarnCallout title="搬迁已登记，下次重启时生效">
          <div>
            {pending ? (
              <>
                下次启动时 NoEnding 会将数据搬迁至{" "}
                <ClickablePath path={pending} />
                ，搬迁完成后才会打开数据库。
              </>
            ) : (
              <>位置变更已登记，但未读到目标路径。在此之前搬迁不会执行。</>
            )}
          </div>
          <div>当前进程仍在使用并写入上方显示的数据库路径。</div>
          <div>
            搬迁仅移动应用专有的 <span className="mono">data/</span>、<span className="mono">runtime/</span>、<span className="mono">logs/</span>；跨磁盘时会完整复制并保留原件。
          </div>
          <div>
            <strong>原目录中的 <span className="mono">workspace/</span> 文件不会被移动</strong>，保留在原处作为普通工作目录。新目录下的 <span className="mono">workspace/</span> 将成为新的默认工作目录。
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

/** 警告提示块。 */
function WarnCallout({ title, children }: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="settings-callout warn">
      <div className="settings-callout-title">{title}</div>
      <div className="settings-callout-body">{children}</div>
    </div>
  );
}

/**
 * 更改数据存储目录：只登记下一次启动要搬去的地方。
 * 搬迁应用专属的 data/、runtime/、logs/，原存储目录下的 workspace/ 用户文件保持原样。
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

  const handleBrowse = async () => {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择新的数据存储目录",
        defaultPath: current.noending_home,
      });
      if (picked && typeof picked === "string") {
        setPath(picked);
        setError("");
      }
    } catch (e) {
      console.error(e);
    }
  };

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError("");
    try {
      onSaved(await api.setNoendingHome(trimmed));
      showToast("迁移已安排：重启 NoEnding 后生效。");
    } catch (e) {
      console.error(e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="更改数据存储目录" onClose={onClose}>
      <label className="field">
        <span>新的存储目录路径</span>
        <div className="row" style={{ gap: 8 }}>
          <input
            type="text"
            className="mono"
            style={{ flex: 1 }}
            value={path}
            placeholder={current.noending_home}
            autoFocus
            onChange={(e) => { setPath(e.target.value); setError(""); }}
            onKeyDown={(e) => { if (submitsOnEnter(e)) void submit(); }}
            disabled={busy}
          />
          <button
            type="button"
            className="btn small"
            onClick={handleBrowse}
            disabled={busy}
          >
            浏览…
          </button>
        </div>
      </label>
      <div className="settings-row-hint" style={{ marginBottom: 12, wordBreak: "break-word" }}>
        当前存储目录：<span className="mono" style={{ wordBreak: "break-all" }}>{current.noending_home}</span>
        （{homeSourceLabel(current.home_source)}）。支持 <span className="mono">~</span> 开头的写法；
        相对路径会被拒绝，请提供绝对路径。
      </div>

      <WarnCallout title="迁移机制说明">
        <div>
          此操作仅<strong>登记迁移计划</strong>：重启 NoEnding 后，会在打开数据库之前完成数据搬迁，
          迁移成功后才会切换到新位置。因此这一项<strong>需要重启应用后生效</strong>。
        </div>
        <div>
          搬迁的内容包含应用专属的 <span className="mono">data/</span>（数据库）、<span className="mono">runtime/</span>（运行缓存）与 <span className="mono">logs/</span>（日志）。
        </div>
        <div>
          <strong>原目录下的 <span className="mono">workspace/</span> 用户代码文件不会被移动或删除</strong>：
          它们将完整保留在原处，后续仍作为普通工作目录识别。
        </div>
        <div>
          新存储目录下的 <span className="mono">workspace/</span> 会自动成为新的默认工作目录。
        </div>
        {current.home_source === "explicit_env" && (
          <div>
            <strong>注意</strong>：环境变量 <span className="mono">NOENDING_HOME</span> 仍具有更高优先级。在取消该环境变量前，重启仍将优先使用其指定的目录。
          </div>
        )}
      </WarnCallout>

      {sameAsCurrent && (
        <div className="muted small" style={{ marginTop: 10 }}>
          这已经是当前位置，无需更改。
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

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={!canSubmit} onClick={submit}>
          {busy ? "登记中…" : "登记并在下次启动迁移"}
        </button>
      </div>
    </Modal>
  );
}
