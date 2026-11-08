import { useState } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import { viewState } from "../hooks/useViewState";
import { DetailLayout, DetailPanelProvider } from "./DetailLayout";
import { DetailSection } from "./DetailSection";
import SummaryContent, { SummaryControls, useSummaryExpansion } from "../features/sessions/SummaryContent";

beforeEach(() => { viewState.clear(); localStorage.clear(); });
afterEach(() => { cleanup(); viewState.clear(); localStorage.clear(); });

it("keeps drafts and in-flight work alive while hiding content, leaving header actions and status available", async () => {
  let finish!: () => void;
  function Work() {
    const [result, setResult] = useState("尚未开始");
    return <><input aria-label="草稿" /><button onClick={async () => {
      setResult("处理中"); await new Promise<void>(resolve => { finish = resolve; }); setResult("操作已完成");
    }}>开始操作</button><p role="status">{result}</p></>;
  }
  let copied = false;
  render(<DetailSection sectionId="summary" label="摘要" title="摘要（2）"
    actions={<button onClick={() => { copied = true; }}>复制</button>} status={<span>待整理 3 条</span>}><Work /></DetailSection>);
  fireEvent.change(screen.getByRole("textbox", { name: "草稿" }), { target: { value: "保留的文字" } });
  fireEvent.click(screen.getByRole("button", { name: "开始操作" }));
  expect(screen.getByRole("status").textContent).toBe("处理中");
  screen.getByRole("textbox", { name: "草稿" }).focus();
  fireEvent.click(screen.getByRole("button", { name: "摘要（2）" }));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "摘要（2）" }));
  expect(screen.queryByRole("textbox", { name: "草稿" })).toBeNull();
  expect(screen.getByRole("heading", { name: "摘要（2）" })).toBeTruthy();
  expect(screen.getByText("待整理 3 条")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "复制" }));
  expect(copied).toBe(true);
  expect(screen.getByRole("button", { name: "摘要（2）" }).getAttribute("aria-expanded")).toBe("false");
  await act(async () => finish());
  fireEvent.click(screen.getByRole("button", { name: "摘要（2）" }));
  expect((screen.getByRole("textbox", { name: "草稿" }) as HTMLInputElement).value).toBe("保留的文字");
  expect(screen.getByRole("status").textContent).toBe("操作已完成");
});

function Page({ kind, object }: { kind: string; object: string }) {
  return <DetailPanelProvider kind={kind}>
    <DetailSection sectionId="summary" label="摘要" title="摘要"><p>{object}的摘要</p></DetailSection>
    <DetailSection sectionId="properties" label="属性" title="属性"><input aria-label="属性内容" /></DetailSection>
  </DetailPanelProvider>;
}
it("restores preferences for the same page and card types across objects and restarts, keeping other types independent", () => {
  const first = render(<Page kind="task" object="甲" />);
  expect(screen.getByText("甲的摘要")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "摘要" }));
  first.unmount(); viewState.clear();
  const second = render(<Page kind="task" object="乙" />);
  expect(screen.getByRole("button", { name: "摘要" }).getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByRole("textbox", { name: "属性内容" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "摘要" }));
  expect(screen.getByText("乙的摘要")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "摘要" }));
  second.unmount();
  const collaboration = render(<Page kind="collaboration" object="丙" />);
  expect(screen.getByRole("button", { name: "摘要" }).getAttribute("aria-expanded")).toBe("true");
  collaboration.unmount();
  render(<Page kind="project" object="丁" />);
  expect(screen.getByRole("button", { name: "摘要" }).getAttribute("aria-expanded")).toBe("true");
});

it("allows an editor action to reveal the body of an initially collapsed card", () => {
  render(<DetailSection sectionId="items" label="事项" title="事项" defaultExpanded={false}
    actions={({ expand }) => <button onClick={expand}>新增事项</button>}><input aria-label="事项标题" /></DetailSection>);
  expect(screen.queryByRole("textbox", { name: "事项标题" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "新增事项" }));
  expect(screen.getByRole("textbox", { name: "事项标题" })).toBeTruthy();
});

it("reveals the card when expanding details but leaves a collapsed card closed when collapsing details", () => {
  const summary = { items: [{ id: "decision", title: "决定", description: "", details: ["决定依据"] }] };
  function Card() {
    const expansion = useSummaryExpansion(summary);
    return <DetailSection sectionId="summary" label="摘要" title="摘要" defaultExpanded={false}
      actions={<SummaryControls expansion={expansion} />}><SummaryContent expansion={expansion}>{summary}</SummaryContent></DetailSection>;
  }
  render(<Card />);
  fireEvent.click(screen.getByRole("button", { name: "全部展开" }));
  expect(screen.getByRole("button", { name: "摘要" }).getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByRole("list").textContent).toBe("决定依据");
  fireEvent.click(screen.getByRole("button", { name: "摘要" }));
  fireEvent.click(screen.getByRole("button", { name: "全部收起" }));
  expect(screen.getByRole("button", { name: "摘要" }).getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: "摘要" }));
  expect(screen.queryByRole("list")).toBeNull();
});


it.each([["task", "collaboration", "project"], ["session", "terminal", "task"]])(
  "shares %s right-hand card preferences with %s while preserving independent main sections and other page groups",
  (firstKind, siblingKind, otherKind) => {
    function SharedPage({ kind }: { kind: string }) {
      return <DetailPanelProvider kind={kind}><DetailLayout panel={
        <DetailSection sectionId="summary" label="右栏信息" title="右栏信息"><input aria-label="右栏内容" defaultValue="保留的内容" /></DetailSection>
      }><DetailSection sectionId="summary" label="主区摘要" title="主区摘要"><p>主区正文</p></DetailSection></DetailLayout></DetailPanelProvider>;
    }
    const first = render(<SharedPage kind={firstKind} />);
    fireEvent.click(screen.getByRole("button", { name: "右栏信息" }));
    fireEvent.click(screen.getByRole("button", { name: "主区摘要" }));
    first.rerender(<SharedPage kind={siblingKind} />);
    expect(screen.getByRole("button", { name: "右栏信息" }).getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByRole("button", { name: "主区摘要" }).getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "右栏信息" }));
    expect((screen.getByRole("textbox", { name: "右栏内容" }) as HTMLInputElement).value).toBe("保留的内容");
    fireEvent.click(screen.getByRole("button", { name: "右栏信息" }));
    first.unmount(); viewState.clear();
    const second = render(<SharedPage kind={firstKind} />);
    expect(screen.getByRole("button", { name: "右栏信息" }).getAttribute("aria-expanded")).toBe("false");
    second.unmount();
    render(<SharedPage kind={otherKind} />);
    expect(screen.getByRole("textbox", { name: "右栏内容" })).toBeTruthy();
  },
);
