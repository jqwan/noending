import { useEffect, useState } from "react";
import type { LaunchResult } from "../../types";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";

/**
 * What happened after a launch: command, context file, delivered bundle.
 *
 * 「注入了什么」按这次启动**实际投递**的内容决定显隐：不注入时
 * markdown 为空，这块内容直接不存在，而不是显示零计数或「未注入」占位。
 * 用实际结果而不是当前设置判断，因为设置可能在启动之后、弹窗关闭之前被改过。
 */
export default function LaunchResultModal({ result, onClose }: {
  result: LaunchResult;
  onClose: () => void;
}) {
  const delivered = result.bundle.markdown.trim().length > 0;
  return (
    <Modal title="启动详情" onClose={onClose}>
      <div className="badge accent" style={{ marginBottom: 12 }}>已通过 {result.launched_via} 启动</div>
      <p className="small">{result.note}</p>
      <h3>执行的命令</h3>
      <div className="card mono small">{result.command_line}</div>
      {delivered && (
        <>
          <h3>注入的 Context</h3>
          <div className="card mono small" style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{result.bundle.markdown}</div>
        </>
      )}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
        <button className="btn primary" onClick={onClose}>关闭</button>
      </div>
    </Modal>
  );
}

let openDetails: ((r: LaunchResult) => void) | null = null;

/** 从任何调用点打开「启动详情」— host 挂在 AppShell，页面卸载也不受影响。 */
export function showLaunchDetails(result: LaunchResult) {
  openDetails?.(result);
}

/** 常规模式：New / Resume 成功只弹 toast，「查看详情」是可选入口。 */
export function announceLaunch(verb: string, result: LaunchResult) {
  showToast(`已通过 ${result.launched_via} ${verb}`, {
    label: "查看详情",
    onClick: () => showLaunchDetails(result),
  });
}

/** AppShell 挂载的全局详情入口（launch diagnostics / Context bundle 预览）。 */
export function LaunchDetailsHost() {
  const [result, setResult] = useState<LaunchResult | null>(null);

  useEffect(() => {
    openDetails = setResult;
    return () => {
      openDetails = null;
    };
  }, []);

  if (!result) return null;
  return <LaunchResultModal result={result} onClose={() => setResult(null)} />;
}
