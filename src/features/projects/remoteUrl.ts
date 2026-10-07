/**
 * 把各协议形式的远程地址归一成可打开的 https 链接。
 *
 * 存储里是 git config 的原文（仅剥过凭据）：克隆走 SSH 的仓库是 scp 风格的
 * `git@host:path`，走 http(s) 的原样。展示层统一成 https，协议只在这一层
 * 变换，原文不动。本地路径（`/`、`./`、`C:\`、含反斜杠）与解析不了的返回
 * 原样——点击时后端只放行 http(s)，那是最后一道兜底。
 */
export function httpsRemoteUrl(raw: string): string {
  const trimmed = raw.trim();
  if (
    /^(\/|\.\.?\/|[A-Za-z]:[\\/])/.test(trimmed) ||
    trimmed.includes("\\")
  ) {
    return trimmed;
  }
  // scp 风格：git@host:path（host 不含冒号，path 起于第一个冒号后）。
  const scp = trimmed.match(/^[^/@]+@([^:/]+):(.+)$/);
  if (scp) return `https://${scp[1]}/${scp[2]}`;
  // ssh URL：ssh://[user@]host[:port]/path —— 端口随 https 一起保留。
  const ssh = trimmed.match(/^ssh:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/(.+)$/);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  // git 守护进程协议（罕见）：git://host/path。
  const git = trimmed.match(/^git:\/\/([^/]+)\/(.+)$/);
  if (git) return `https://${git[1]}/${git[2]}`;
  return trimmed;
}
