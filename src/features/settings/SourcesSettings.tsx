import React from "react";
import SourcesView from "../sources/SourcesView";

/** Settings → Session Sources：复用现有 SourcesView（实施方案 §10/§49）。 */
export default function SourcesSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Session Sources</h3>
      <SourcesView embedded />
    </section>
  );
}
