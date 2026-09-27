import { Component, type ReactNode } from "react";
import PageHeader from "../layout/PageHeader";

/**
 * 内容区兜底：页面里任何一次渲染抛错都留在这里，不再把整棵树一起卸载成黑屏。
 * 原始错误照原样显示——没有它，报障只剩「应用黑屏了」这一句。
 *
 * `resetKey`（路由）变化时复位：切到别的页面该看到那一页，而不是上一页留下的兜底。
 * 「重试」只是清掉错误标记：错误态下子树本来就没挂载，复位后自然重新挂载取数。
 */
export default class ErrorBoundary extends Component<
  { children: ReactNode; resetKey?: string },
  { error: unknown }
> {
  state: { error: unknown } = { error: null };

  static getDerivedStateFromError(error: unknown) {
    return { error };
  }

  componentDidUpdate(prev: { resetKey?: string }) {
    if (this.state.error !== null && prev.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error === null) return this.props.children;
    return (
      <div className="main narrow">
        <PageHeader title="这一页出错了">
          <p className="muted small">
            这一页没能画出来，本地数据没有被修改。可以从侧栏切到别的页面，或者重试。
          </p>
          <div className="invite">
            <button className="btn small" onClick={() => this.setState({ error: null })}>
              重试
            </button>
          </div>
          <p className="muted small mono" style={{ marginTop: 14, overflowWrap: "anywhere" }}>
            {String(this.state.error)}
          </p>
        </PageHeader>
      </div>
    );
  }
}
