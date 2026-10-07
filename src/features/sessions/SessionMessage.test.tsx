import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionMessage, { looksLikeMarkdown } from "./SessionMessage";
import type { SessionMessageData } from "./SessionMessage";

vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));

afterEach(cleanup);

const LONG = "第一段说明。".repeat(50) + "结尾标记";
const MD = "## 小节\n\n- 一\n- 二";

function msg(content: string, role: SessionMessageData["role"] = "assistant"): SessionMessageData {
  return {
    sequence: 7,
    role,
    content,
    ts: null,
    who: role === "user" ? "用户" : "Codex",
    turnFinal: true,
  };
}

function renderMessage(content: string, role: SessionMessageData["role"] = "assistant") {
  return render(<SessionMessage msg={msg(content, role)} />);
}

it("列表里截断，点开弹窗才给全文", () => {
  renderMessage(LONG);

  // 列表只显示截断内容，点击气泡后才展示全文。
  expect(screen.queryByText("结尾标记")).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();

  fireEvent.click(screen.getByText(/^第一段说明/));

  const dialog = screen.getByRole("dialog");
  expect(within(dialog).getByText(/结尾标记/)).toBeTruthy();
});

it("气泡里直接显示 Markdown 预览，不露出源码", () => {
  const { container } = renderMessage(MD);
  const body = container.querySelector(".event .body")!;

  expect(body.querySelector("h2")?.textContent).toBe("小节");
  expect(body.querySelectorAll("li")).toHaveLength(2);
  expect(body.textContent).not.toContain("##");

  // 悬浮 / 点开的作用对象是气泡本身，不是整行。
  expect(body.className).toContain("clickable");
  expect(body.getAttribute("role")).toBe("button");
});

it("没有可读文本的消息不开弹窗", () => {
  renderMessage("   ");

  expect(screen.getByText("（该消息没有可读文本）")).toBeTruthy();
  fireEvent.click(screen.getByText("（该消息没有可读文本）"));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("弹窗默认就停在预览，且不注入原始 HTML", () => {
  // 转录是我们不控制的内容：raw HTML 不渲染，a / img 都退化成文字。
  renderMessage("# 标题\n\n<script>window.__pwned = 1</script>\n\n[链接](https://example.invalid/a) ![图](https://example.invalid/b.png)");
  fireEvent.click(screen.getByRole("button"));

  const dialog = screen.getByRole("dialog");
  expect(within(dialog).getByRole("button", { name: "预览" }).getAttribute("aria-pressed")).toBe("true");
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
