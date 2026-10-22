import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { api } from "../../api";
import { afterEach, expect, it, vi } from "vitest";
import * as common from "../../components/common";
import * as toast from "../../components/Toast";
import { useSummaryEditor } from "./useSummaryEditor";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "./SummaryContent";
import type { SummaryDocument, SummaryEditResult } from "../../types";

function SummaryPreview({ children }: { children: SummaryDocument }) {
  const expansion = useSummaryExpansion(children);
  return <><header><SummaryControls expansion={expansion} /></header><SummaryContent expansion={expansion}>{children}</SummaryContent></>;
}

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const summary: SummaryDocument = { items: [
  { title: "路径同步", description: "保留已有目录。", title_locked: false, details: ["保留 **原始文件**。", "使用 `ne status` 检查连接。"] },
  { title: "后续工作", description: "", title_locked: false, details: [] },
] };
it("keeps titles and descriptions visible while only details start collapsed", () => {
  render(<SummaryPreview>{summary}</SummaryPreview>);
  const topic = screen.getByRole("region", { name: "路径同步" });
  within(topic).getByRole("heading", { name: "路径同步" });
  within(topic).getByText("保留已有目录。");
  expect(within(topic).queryByRole("list")).toBeNull();
  const toggle = within(topic).getByRole("button", { name: "展开路径同步的细则" });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(within(topic).getByRole("list").id).toBe(toggle.getAttribute("aria-controls"));
  expect(screen.getByText("原始文件").tagName).toBe("STRONG");
  expect(screen.getByText("ne status").tagName).toBe("CODE");
  fireEvent.click(toggle);
  expect(within(topic).queryByRole("list")).toBeNull();
  within(topic).getByText("保留已有目录。");
  const simple = screen.getByRole("region", { name: "后续工作" });
  expect(within(simple).queryByRole("button", { name: /细则/ })).toBeNull();
});
it("expands and collapses all detail lists, including after an individual toggle", () => {
  render(<SummaryPreview>{{ items: [...summary.items, { title: "验证范围", description: "待验证", title_locked: false, details: ["Windows 仍待验证。"] }] }}</SummaryPreview>);
  fireEvent.click(screen.getByRole("button", { name: "展开路径同步的细则" }));
  expect(screen.getAllByRole("list")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "全部展开" }));
  expect(screen.getAllByRole("list")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "全部收起" }));
  expect(screen.queryByRole("list")).toBeNull();
  screen.getByText("保留已有目录。");
  screen.getByText("待验证");
});
it("does not offer bulk detail controls when no item has details", () => {
  render(<SummaryPreview>{{ items: [summary.items[1]] }}</SummaryPreview>);
  expect(screen.queryByRole("button", { name: /全部展开|全部收起/ })).toBeNull();
});
it("retains expansion on identical reads and resets it when the summary is reorganized", () => {
  const { rerender } = render(<SummaryPreview>{summary}</SummaryPreview>);
  fireEvent.click(screen.getByRole("button", { name: "展开路径同步的细则" }));
  rerender(<SummaryPreview>{JSON.parse(JSON.stringify(summary)) as SummaryDocument}</SummaryPreview>);
  screen.getByRole("list");
  rerender(<SummaryPreview>{{ items: [{ title: "合并后的事项", description: "### 内容中的标题", title_locked: false, details: ["新的细则"] }] }}</SummaryPreview>);
  expect(screen.queryByText("路径同步")).toBeNull();
  screen.getByText("内容中的标题");
  expect(screen.queryByRole("list")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "展开合并后的事项的细则" }));
  expect(screen.getByRole("list").textContent).toBe("新的细则");
});
it.each([true, false])("copies one complete item even with details collapsed (success=%s)", async success => {
  const copy = vi.spyOn(common, "copyToClipboard").mockResolvedValue(success);
  const notify = vi.spyOn(toast, "showToast").mockImplementation(() => {});
  render(<SummaryPreview>{summary}</SummaryPreview>);
  fireEvent.click(screen.getByRole("button", { name: "复制事项：路径同步" }));
  await waitFor(() => expect(notify).toHaveBeenCalledWith(success ? "已复制事项" : "复制失败"));
  expect(copy).toHaveBeenCalledWith("### 路径同步\n\n保留已有目录。\n\n- 保留 **原始文件**。\n- 使用 `ne status` 检查连接。");
  expect(screen.queryByRole("list")).toBeNull();
});
it("exports the same structured content as readable Markdown", () => {
  expect(summaryMarkdown(summary)).toBe("### 路径同步\n\n保留已有目录。\n\n- 保留 **原始文件**。\n- 使用 `ne status` 检查连接。\n\n### 后续工作");
});

it.each(["session","task","project"] as const)("persists and restores title locks for %s while keeping the body and expanded details", async kind => {
  let saved: SummaryEditResult = {revision:7,summary:JSON.parse(JSON.stringify(summary))};
  const save = vi.spyOn(api,"setSummaryTitleLock").mockImplementation(async (_kind,_id,revision,itemNumber,locked) => {
    expect(revision).toBe(saved.revision);
    saved = {revision:revision+1,summary:{items:saved.summary.items.map((item,index) => index===itemNumber-1 ? {...item,title_locked:locked} : item)}};
    return saved;
  });
  function Editor() {
    const [view,setView] = useState(saved);
    const expansion = useSummaryExpansion(view.summary);
    const editor = useSummaryEditor({summary:view.summary,target:{kind,id:"target",revision:view.revision},onChange:setView});
    return <SummaryContent expansion={expansion} editor={editor}>{view.summary}</SummaryContent>;
  }
  const first = render(<Editor />);
  fireEvent.click(screen.getByRole("button",{name:"展开路径同步的细则"}));
  fireEvent.click(screen.getByRole("button",{name:"锁定标题：路径同步"}));
  await waitFor(() => expect(screen.getByRole("button",{name:"解锁标题：路径同步"}).getAttribute("aria-pressed")).toBe("true"));
  expect(save).toHaveBeenCalledWith(kind,"target",7,1,true);
  screen.getByText("保留已有目录。");
  screen.getByRole("list");
  expect(saved.summary.items[0].details).toEqual(["保留 **原始文件**。","使用 `ne status` 检查连接。"]);
  first.unmount();
  render(<Editor />);
  screen.getByRole("button",{name:"解锁标题：路径同步"});
  fireEvent.click(screen.getByRole("button",{name:"解锁标题：路径同步"}));
  await waitFor(() => expect(screen.getByRole("button",{name:"锁定标题：路径同步"}).getAttribute("aria-pressed")).toBe("false"));
  expect(save).toHaveBeenCalledWith(kind,"target",8,1,false);
});

it("keeps the current lock state when a stale title-lock request is rejected", async () => {
  vi.spyOn(api,"setSummaryTitleLock").mockRejectedValue("摘要版本已变化");
  const notify = vi.spyOn(toast,"showToast").mockImplementation(() => {});
  const changed = vi.fn();
  function Editor() {
    const expansion = useSummaryExpansion(summary);
    const editor = useSummaryEditor({summary,target:{kind:"session",id:"s",revision:1},onChange:changed});
    return <SummaryContent expansion={expansion} editor={editor}>{summary}</SummaryContent>;
  }
  render(<Editor />);
  fireEvent.click(screen.getByRole("button",{name:"锁定标题：路径同步"}));
  await waitFor(() => expect(notify).toHaveBeenCalledWith(expect.stringContaining("摘要版本已变化")));
  expect(screen.getByRole("button",{name:"锁定标题：路径同步"}).getAttribute("aria-pressed")).toBe("false");
  expect(changed).not.toHaveBeenCalled();
});

function Editable({ initial, kind = "session", id = "target", onSaved = () => {} }: {
  initial: SummaryEditResult;
  kind?: "session" | "task" | "project";
  id?: string;
  onSaved?: (result: SummaryEditResult) => void;
}) {
  const [view, setView] = useState(initial);
  const expansion = useSummaryExpansion(view.summary);
  const editor = useSummaryEditor({ summary: view.summary, target: { kind, id, revision: view.revision },
    onChange: (result, removedIndex) => { expansion.preserve(result.summary, removedIndex); setView(result); onSaved(result); } });
  return <>
    <header><SummaryControls expansion={expansion} editor={editor} />
      <button disabled={editor.editing}>更新摘要</button>
    </header>
    <SummaryContent expansion={expansion} editor={editor}>{view.summary}</SummaryContent>
    {!view.summary.items.length && !editor.editing && <p>暂无摘要</p>}
  </>;
}

it.each(["session", "task", "project"] as const)("edits an item and its detail list for %s, preserving other items after remount", async kind => {
  let saved: SummaryEditResult = { revision: 4, summary: structuredClone(summary) };
  vi.spyOn(api, "saveSummary").mockImplementation(async (_kind, _id, revision, content) => {
    expect(revision).toBe(4);
    saved = { revision: 5, summary: content };
    return saved;
  });
  const first = render(<Editable initial={saved} kind={kind} />);
  fireEvent.click(screen.getByRole("button", { name: "展开路径同步的细则" }));
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：路径同步" }));
  const form = screen.getByRole("form", { name: "编辑事项：路径同步" });
  expect(document.activeElement).toBe(within(form).getByRole("textbox", { name: "事项标题" }));
  expect(screen.getByRole("button", { name: "更新摘要" }).hasAttribute("disabled")).toBe(true);
  expect(screen.getByRole("button", { name: "编辑事项：后续工作" }).hasAttribute("disabled")).toBe(true);
  fireEvent.change(within(form).getByRole("textbox", { name: "事项标题" }), { target: { value: "路径校验" } });
  fireEvent.change(within(form).getByRole("textbox", { name: "事项描述" }), { target: { value: "已完成校验。" } });
  fireEvent.click(within(form).getByRole("button", { name: "移除细则 2" }));
  fireEvent.change(within(form).getByRole("textbox", { name: "细则 1" }), { target: { value: "保留源文件和关联。" } });
  fireEvent.click(within(form).getByRole("button", { name: "添加细则" }));
  expect(document.activeElement).toBe(within(form).getByRole("textbox", { name: "细则 2" }));
  fireEvent.change(within(form).getByRole("textbox", { name: "细则 2" }), { target: { value: "Windows 也已验证。" } });
  fireEvent.click(within(form).getByRole("button", { name: "保存" }));
  await screen.findByRole("heading", { name: "路径校验" });
  expect(screen.getByRole("list").textContent).toBe("保留源文件和关联。Windows 也已验证。");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "编辑事项：路径校验" }));
  expect(screen.queryByRole("form")).toBeNull();
  first.unmount();
  render(<Editable initial={saved} kind={kind} />);
  screen.getByRole("heading", { name: "路径校验" });
  screen.getByText("已完成校验。");
  screen.getByRole("heading", { name: "后续工作" });
  fireEvent.click(screen.getByRole("button", { name: "展开路径校验的细则" }));
  expect(screen.getByRole("list").textContent).toBe("保留源文件和关联。Windows 也已验证。");
});

it("adds the first item, cancels a draft, and clears the last item only after confirmation", async () => {
  const save = vi.spyOn(api, "saveSummary").mockImplementation(async (_kind, _id, revision, content) => ({ revision: revision + 1, summary: content }));
  render(<Editable initial={{ revision: 0, summary: { items: [] } }} />);
  screen.getByText("暂无摘要");
  fireEvent.click(screen.getByRole("button", { name: "新增事项" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项标题" }), { target: { value: "未保存草稿" } });
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(save).not.toHaveBeenCalled();
  screen.getByText("暂无摘要");
  fireEvent.click(screen.getByRole("button", { name: "新增事项" }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  expect(screen.getByRole("alert").textContent).toContain("请填写事项标题");
  fireEvent.change(screen.getByRole("textbox", { name: "事项标题" }), { target: { value: "新增的结论" } });
  fireEvent.keyDown(screen.getByRole("textbox", { name: "事项标题" }), { key: "Enter", ctrlKey: true });
  await screen.findByRole("heading", { name: "新增的结论" });
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：新增的结论" }));
  fireEvent.click(screen.getByRole("button", { name: "删除事项" }));
  screen.getByText("删除后摘要将为空。");
  fireEvent.click(screen.getByRole("button", { name: "保留事项" }));
  screen.getByRole("textbox", { name: "事项标题" });
  fireEvent.click(screen.getByRole("button", { name: "删除事项" }));
  fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
  await screen.findByText("暂无摘要");
  expect(screen.queryByRole("heading", { name: "新增的结论" })).toBeNull();
});

it("keeps a locked title read-only and undeletable while saving body changes", async () => {
  vi.spyOn(api, "saveSummary").mockImplementation(async (_kind, _id, revision, content) => ({ revision: revision + 1, summary: content }));
  render(<Editable initial={{ revision: 2, summary: { items: [{ ...summary.items[0], title_locked: true }] } }} />);
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：路径同步" }));
  expect(screen.getByRole("textbox", { name: "事项标题" }).hasAttribute("readonly")).toBe(true);
  expect(screen.getByRole("button", { name: "删除事项" }).hasAttribute("disabled")).toBe(true);
  fireEvent.change(screen.getByRole("textbox", { name: "事项描述" }), { target: { value: "新的描述" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByText("新的描述");
  screen.getByRole("button", { name: "解锁标题：路径同步" });
});

it("keeps the neighboring item's details expanded when an earlier item is deleted", async () => {
  vi.spyOn(api, "saveSummary").mockImplementation(async (_kind, _id, revision, content) => ({ revision: revision + 1, summary: content }));
  render(<Editable initial={{ revision: 1, summary: { items: [summary.items[0], { ...summary.items[1], details: ["仍需核实 Windows。"] }] } }} />);
  fireEvent.click(screen.getByRole("button", { name: "展开后续工作的细则" }));
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：路径同步" }));
  fireEvent.click(screen.getByRole("button", { name: "删除事项" }));
  fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
  await waitFor(() => expect(screen.queryByRole("heading", { name: "路径同步" })).toBeNull());
  expect(screen.getByRole("list").textContent).toBe("仍需核实 Windows。");
  screen.getByRole("button", { name: "收起后续工作的细则" });
});

it("retains the complete draft after save failure and when a newer summary arrives", async () => {
  let reject!: (reason: string) => void;
  vi.spyOn(api, "saveSummary").mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
  const initial = { revision: 1, summary };
  function Concurrent() {
    const [view, setView] = useState(initial);
    const expansion = useSummaryExpansion(view.summary);
    const editor = useSummaryEditor({ summary: view.summary, target: { kind: "session", id: "s", revision: view.revision }, onChange: setView });
    return <>
      <button onClick={() => setView({ revision: 2, summary: { items: [{ ...summary.items[0], title: "其他地方的新结论" }] } })}>外部更新</button>
      <SummaryContent expansion={expansion} editor={editor}>{view.summary}</SummaryContent>
    </>;
  }
  render(<Concurrent />);
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：路径同步" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项描述" }), { target: { value: "用户未提交的描述" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  expect(screen.getByRole("textbox", { name: "事项描述" }).hasAttribute("disabled")).toBe(true);
  reject("磁盘写入失败");
  await screen.findByRole("alert");
  expect((screen.getByRole("textbox", { name: "事项描述" }) as HTMLTextAreaElement).value).toBe("用户未提交的描述");
  fireEvent.click(screen.getByRole("button", { name: "外部更新" }));
  expect(screen.getByRole("alert").textContent).toContain("摘要已在其他地方更新");
  expect((screen.getByRole("textbox", { name: "事项描述" }) as HTMLTextAreaElement).value).toBe("用户未提交的描述");
  expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(true);
  const copy = vi.spyOn(common, "copyToClipboard").mockResolvedValue(true);
  fireEvent.click(screen.getByRole("button", { name: "复制草稿" }));
  await waitFor(() => expect(copy).toHaveBeenCalledWith(expect.stringContaining("用户未提交的描述")));
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  screen.getByRole("heading", { name: "其他地方的新结论" });
});

it("ignores a completed save after switching targets and permits editing the new target", async () => {
  let finish!: (result: SummaryEditResult) => void;
  vi.spyOn(api, "saveSummary").mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const onSaved = vi.fn();
  const view = { revision: 1, summary };
  const page = render(<Editable initial={view} id="first" onSaved={onSaved} />);
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：路径同步" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项描述" }), { target: { value: "第一个会话的草稿" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  page.rerender(<Editable initial={view} id="second" onSaved={onSaved} />);
  expect(screen.queryByRole("form")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "编辑事项：后续工作" }));
  fireEvent.change(screen.getByRole("textbox", { name: "事项描述" }), { target: { value: "第二个会话的草稿" } });
  await act(async () => finish({ revision: 2, summary: { items: [{ ...summary.items[0], description: "旧对象保存完成" }] } }));
  expect(onSaved).not.toHaveBeenCalled();
  expect((screen.getByRole("textbox", { name: "事项描述" }) as HTMLTextAreaElement).value).toBe("第二个会话的草稿");
  expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(false);
});
