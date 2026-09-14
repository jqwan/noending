import React from "react";
import type { LaunchResult } from "../../types";
import { Modal } from "../../components/common";

/** What happened after a launch: command, context file, delivered bundle. */
export default function LaunchResultModal({ result, onClose }: {
  result: LaunchResult;
  onClose: () => void;
}) {
  return (
    <Modal title="已启动" onClose={onClose}>
      <div className="badge accent" style={{ marginBottom: 12 }}>已通过 {result.launched_via} 启动</div>
      <p className="small">{result.note}</p>
      <h3>执行的命令</h3>
      <div className="card mono small">{result.command_line}</div>
      <h3>注入的上下文（约 {result.bundle.approx_tokens} tokens）</h3>
      <div className="card mono small" style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{result.bundle.markdown}</div>
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
        <button className="btn primary" onClick={onClose}>完成</button>
      </div>
    </Modal>
  );
}
