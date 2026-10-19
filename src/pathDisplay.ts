/** Format a filesystem path for display only; keep the original for file operations. */
export function displayPath(path: string): string {
  const windows = path.replace(/\//g, "\\");
  if (/^\\\\\?\\UNC\\/i.test(windows)) return `\\\\${windows.slice(8)}`;
  if (/^\\\\\?\\[a-z]:\\/i.test(windows)) return windows.slice(4);
  if (/^[a-z]:\\/i.test(windows) || /^\\\\[^\\?\.][^\\]*\\/.test(windows)) return windows;
  return path;
}
