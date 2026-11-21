import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import * as common from "../../components/common";
import * as toast from "../../components/Toast";
import SummaryContent, { summaryMarkdown, SummaryControls, useSummaryExpansion } from "./SummaryContent";
import type { SummaryDocument } from "../../types";

function SummaryPreview({ children }: { children: SummaryDocument }) {
  const expansion = useSummaryExpansion(children);
  return <><header><SummaryControls expansion={expansion} /></header><SummaryContent expansion={expansion}>{children}</SummaryContent></>;
}

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const summary: SummaryDocument = { items: [
  { id: "SummaryContent.test-item-1", title: "路径同步", description: "保留已有目录。", details: ["保留 **原始文件**。", "使用 `ne status` 检查连接。"] },
  { id: "SummaryContent.test-item-2", title: "后续工作", description: "", details: [] },
] };
it("toggles one item or all details without hiding headings and descriptions", () => {
  const multi = { items: [...summary.items, { id: "validation", title: "验证范围", description: "待验证", details: ["Windows 仍待验证。"] }] };
  const { rerender } = render(<SummaryPreview>{multi}</SummaryPreview>);
  const topic = within(screen.getByRole("region", { name: "路径同步" }));
  expect(topic.getByText("保留已有目录。")).toBeTruthy();
  expect(topic.queryByRole("list")).toBeNull();
  const toggle = topic.getByRole("button", { name: "展开路径同步的细则" });
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(topic.getByRole("list").id).toBe(toggle.getAttribute("aria-controls"));
  expect(topic.getByText("原始文件").tagName).toBe("STRONG");
  expect(topic.getByText("ne status").tagName).toBe("CODE");
  fireEvent.click(toggle);
  expect(topic.queryByRole("list")).toBeNull();
  fireEvent.click(toggle);
  fireEvent.click(screen.getByRole("button", { name: "全部展开" }));
  expect(screen.getAllByRole("list")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "全部收起" }));
  expect(screen.queryByRole("list")).toBeNull();
  expect(topic.getByText("保留已有目录。")).toBeTruthy();
  expect(screen.getByText("待验证")).toBeTruthy();
  rerender(<SummaryPreview>{{ items: [summary.items[1]] }}</SummaryPreview>);
  expect(screen.getByRole("heading", { name: "后续工作" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /细则|全部展开|全部收起/ })).toBeNull();
});
it("retains expansion on identical reads and resets it when the summary is reorganized", () => {
  const { rerender } = render(<SummaryPreview>{summary}</SummaryPreview>);
  fireEvent.click(screen.getByRole("button", { name: "展开路径同步的细则" }));
  rerender(<SummaryPreview>{JSON.parse(JSON.stringify(summary)) as SummaryDocument}</SummaryPreview>);
  screen.getByRole("list");
  rerender(<SummaryPreview>{{ items: [{ id: "SummaryContent.test-item-4", title: "合并后的事项", description: "### 内容中的标题", details: ["新的细则"] }] }}</SummaryPreview>);
  expect(screen.queryByText("路径同步")).toBeNull();
  screen.getByText("内容中的标题");
  expect(screen.queryByRole("list")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "展开合并后的事项的细则" }));
  expect(screen.getByRole("list").textContent).toBe("新的细则");
});
it("copies collapsed details and reports failed and successful attempts", async () => {
  const copy = vi.spyOn(common, "copyToClipboard").mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  const notify = vi.spyOn(toast, "showToast").mockImplementation(() => {});
  render(<SummaryPreview>{summary}</SummaryPreview>);
  const button = screen.getByRole("button", { name: "复制事项：路径同步" });
  fireEvent.click(button);
  await waitFor(() => expect(notify).toHaveBeenLastCalledWith("复制失败"));
  fireEvent.click(button);
  await waitFor(() => expect(notify).toHaveBeenLastCalledWith("已复制事项"));
  expect(copy).toHaveBeenCalledWith("### 路径同步\n\n保留已有目录。\n\n- 保留 **原始文件**。\n- 使用 `ne status` 检查连接。");
  expect(screen.queryByRole("list")).toBeNull();
});
it("exports the same structured content as readable Markdown", () => {
  expect(summaryMarkdown(summary)).toBe("### 路径同步\n\n保留已有目录。\n\n- 保留 **原始文件**。\n- 使用 `ne status` 检查连接。\n\n### 后续工作");
});

it("keeps expanded details attached to the item ID after renaming and reordering", () => {
  const {rerender} = render(<SummaryPreview>{summary}</SummaryPreview>);
  fireEvent.click(screen.getByRole("button", {name:"展开路径同步的细则"}));
  rerender(<SummaryPreview>{{items:[summary.items[1], {...summary.items[0],title:"路径处理已更新"}]}}</SummaryPreview>);
  const item = screen.getByRole("region",{name:"路径处理已更新"});
  expect(within(item).getByRole("button",{name:"收起路径处理已更新的细则"}).getAttribute("aria-expanded")).toBe("true");
  within(item).getByRole("list");
});
