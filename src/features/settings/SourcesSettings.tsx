import SourcesView from "../sources/SourcesView";

/** @deprecated 设置项已简化，会话来源已统一迁移至侧边栏 features/agents/AgentsView。保留此模块仅作向下兼容。 */
export default function SourcesSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>会话来源</h3>
      <SourcesView />
    </section>
  );
}
