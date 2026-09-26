import SourcesView from "../sources/SourcesView";

/** 设置 → Session 来源：复用现有 SourcesView。 */
export default function SourcesSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>会话来源</h3>
      <SourcesView />
    </section>
  );
}
