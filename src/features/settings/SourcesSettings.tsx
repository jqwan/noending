import React from "react";
import SourcesView from "../sources/SourcesView";

/** 设置 → Session 来源（方案 §2.1 词表）：复用现有 SourcesView（实施方案 §10/§49）。 */
export default function SourcesSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Session 来源</h3>
      <SourcesView embedded />
    </section>
  );
}
