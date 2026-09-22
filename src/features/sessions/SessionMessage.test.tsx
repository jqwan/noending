import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionMessage, { looksLikeMarkdown } from "./SessionMessage";

vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));

afterEach(cleanup);

const LONG = "第一段说明。".repeat(50) + "结尾标记";

function renderMessage(text: string, kind = "assistant_message") {
  render(<SessionMessage msg={{ sequence: 7, kind, text, ts: null, who: "Codex" }} />);
}

it("列表里截断，点开弹窗才给全文", () => {
  renderMessage(LONG);

  // 列表里看不到结尾，也看不到「展开全文」那个就地展开的入口。
  expect(screen.queryByText("结尾标记")).toBeNull();
  expect(screen.queryByText("展开全文")).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();

  fireEvent.click(screen.getByText(/^第一段说明/));

  const dialog = screen.getByRole("dialog");
  expect(within(dialog).getByText(/结尾标记/)).toBeTruthy();
});

it("没有可读文本的事件不开弹窗", () => {
  renderMessage("   ", "system");

  expect(screen.getByText("（该事件没有可读文本）")).toBeTruthy();
  fireEvent.click(screen.getByText("（该事件没有可读文本）"));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("预览不注入原始 HTML，也不产生会外连的元素", () => {
  // 转录是我们不控制的内容：raw HTML 不渲染，a / img 都退化成文字。
  renderMessage("# 标题\n\n<script>window.__pwned = 1</script>\n\n[链接](https://example.invalid/a) ![图](https://example.invalid/b.png)");

  fireEvent.click(screen.getByText(/^# 标题/));
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "预览" }));

  const dialog = screen.getByRole("dialog");
  expect(dialog.querySelector("h1")?.textContent).toBe("标题");
  expect(dialog.querySelector("script")).toBeNull();
  expect(dialog.querySelector("a")).toBeNull();
  expect(dialog.querySelector("img")).toBeNull();
  expect(within(dialog).getByText("链接")).toBeTruthy();
});

it("只有看起来像 Markdown 才给预览页签", () => {
  renderMessage("就是一段普通的话，没有别的。");
  fireEvent.click(screen.getByText("就是一段普通的话，没有别的。"));

  expect(within(screen.getByRole("dialog")).queryByRole("button", { name: "预览" })).toBeNull();
});

it("looksLikeMarkdown 认代码块 / 标题 / 列表，不认普通句子", () => {
  expect(looksLikeMarkdown("```ts\nconst a = 1;\n```")).toBe(true);
  expect(looksLikeMarkdown("## 小节")).toBe(true);
  expect(looksLikeMarkdown("- 一条")).toBe(true);
  expect(looksLikeMarkdown("就是一段普通的话，没有别的。")).toBe(false);
});

it("用户消息右、Agent 左，技术事件保持整行", () => {
  // 左右对齐全靠这三个类（§36.23）；CSS 挂了测试也看不出来，所以在这里钉住类名。
  const { container } = render(
    <>
      <SessionMessage msg={{ sequence: 1, kind: "user_message", text: "提问", ts: null, who: "用户" }} />
      <SessionMessage msg={{ sequence: 2, kind: "assistant_message", text: "回答", ts: null, who: "Codex" }} />
      <SessionMessage msg={{ sequence: 3, kind: "system", text: "系统提示", ts: null, who: "系统" }} />
    </>,
  );

  const rows = [...container.querySelectorAll(".event")].map((el) => el.className);
  expect(rows[0]).toContain("is-user");
  expect(rows[1]).toContain("is-agent");
  expect(rows[2]).toContain("is-tech");
  expect(rows[2]).not.toContain("is-user");
});
