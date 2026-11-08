import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import PageHeader, { PageHeaderHost } from "./PageHeader";
import Icon from "../components/Icon";
import { DetailLayout, DetailPanelProvider } from "../components/DetailLayout";
import { viewState } from "../hooks/useViewState";

afterEach(() => { cleanup(); viewState.clear(); localStorage.clear(); });

describe("PageHeader", () => {
  it("keeps navigation focus across different subpages and directs the pane toggle to the current page", () => {
    const navigate = vi.fn(), firstAction = vi.fn(), nextAction = vi.fn();
    function Navigation() { return <button onClick={navigate}>会话详情</button>; }
    function First() { return <DetailPanelProvider kind="first"><div className="main"><PageHeader title="会话" navigation={<Navigation />} actions={<button onClick={firstAction}>同步</button>} /><DetailLayout panel={<p>详情属性</p>}>详情正文</DetailLayout></div></DetailPanelProvider>; }
    function Next() { return <DetailPanelProvider kind="next"><div className="main"><PageHeader title="会话" navigation={<Navigation />} actions={<button onClick={nextAction}>同步</button>} /><DetailLayout panel={<p>历史属性</p>}>历史正文</DetailLayout></div></DetailPanelProvider>; }
    const view = render(<PageHeaderHost><First /></PageHeaderHost>);
    const navigation = screen.getByRole("button", { name: "会话详情" });
    const sync = screen.getByRole("button", { name: "同步" });
    navigation.focus();
    view.rerender(<PageHeaderHost><Next /></PageHeaderHost>);
    expect(screen.getByRole("button", { name: "会话详情" })).toBe(navigation);
    expect(document.activeElement).toBe(navigation);
    expect(screen.getByRole("button", { name: "同步" })).toBe(sync);
    fireEvent.click(sync);
    expect(nextAction).toHaveBeenCalledOnce();
    expect(firstAction).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
    expect(screen.getByRole("button", { name: "展开详情栏" })).toBeTruthy();
    expect(screen.getByText("历史属性").closest("aside")?.hidden).toBe(true);
  });
  it("keeps the title out of the window drag region", () => {
    const { container } = render(<PageHeader title="新建会话" />);

    // 这条 band 吸在应用标题栏上，自己得是可拖的：tauri 的 drag.js 只认这个属性。
    expect(container.querySelector(".page-head")?.getAttribute("data-tauri-drag-region")).toBe("deep");
    // 标题是内容不是窗口边框。少了 ="false"，按一下标题就把整个窗口带走，
    // 双击还会最大化——tauri 沿 composed path 上溯，遇到 ="false" 就停。
    expect(container.querySelector(".page-title")?.getAttribute("data-tauri-drag-region")).toBe("false");
  });

  it("keeps navigation, operations and the sidebar toggle in keyboard order while preserving action focus on title changes", () => {
    const navigate = vi.fn(), edit = vi.fn();
    const header = (title: string) => <DetailPanelProvider kind="header-test"><PageHeader
      title={title} icon={<Icon name="tasks" />}
      navigation={<button onClick={navigate}>历史消息</button>}
      actions={<button onClick={edit}>编辑任务</button>} />
    </DetailPanelProvider>;
    const page = render(header("原始标题"));
    expect(screen.getAllByRole("button").map(button => button.getAttribute("aria-label") ?? button.textContent))
      .toEqual(["历史消息", "编辑任务", "收起详情栏"]);
    const button = screen.getByRole("button", { name: "编辑任务" });
    button.focus();
    page.rerender(header("更新后的完整标题"));
    expect(document.activeElement).toBe(button);
    expect(screen.getByRole("heading", { name: "更新后的完整标题" }).getAttribute("title")).toBe("更新后的完整标题");
    fireEvent.click(screen.getByRole("button", { name: "历史消息" }));
    fireEvent.click(button);
    expect(navigate).toHaveBeenCalledOnce();
    expect(edit).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
    expect(screen.getByRole("button", { name: "展开详情栏" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "编辑任务" })).toBe(button);
    fireEvent.click(screen.getByRole("button", { name: "展开详情栏" }));
    expect(screen.getByRole("button", { name: "编辑任务" })).toBe(button);
  });
});
