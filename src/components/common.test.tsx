import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { Modal } from "./common";
afterEach(cleanup);
it("keeps keyboard focus inside the dialog and restores the opener", () => {
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  const view = render(<Modal title="编辑" onClose={() => {}}><button>取消</button><button>保存</button></Modal>);
  fireEvent.keyDown(document, { key: "Tab" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "关闭弹窗" }));
  fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
  expect(document.activeElement).toBe(screen.getByText("保存"));
  view.unmount();
  expect(document.activeElement).toBe(opener);
  opener.remove();
});
it("only closes the topmost dialog on Escape", () => {
  const outer = vi.fn(); const inner = vi.fn();
  render(<Modal title="外层" onClose={outer}><Modal title="内层" onClose={inner}>内容</Modal></Modal>);
  fireEvent.keyDown(document, { key: "Escape" });
  expect(inner).toHaveBeenCalledOnce();
  expect(outer).not.toHaveBeenCalled();
});

it("closes from the shared header button", () => {
  const close = vi.fn();
  render(<Modal title="编辑" onClose={close}>内容</Modal>);
  fireEvent.click(screen.getByRole("button", { name: "关闭弹窗" }));
  expect(close).toHaveBeenCalledOnce();
});
