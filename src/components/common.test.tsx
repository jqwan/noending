import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { Modal, submitsOnEnter } from "./common";
import CustomSelect from "./CustomSelect";
afterEach(cleanup);

it("takes Enter as submit only when no IME is mid-composition", () => {
  const press = (key: string, isComposing: boolean, keyCode: number) =>
    submitsOnEnter({ key, nativeEvent: { isComposing, keyCode } });

  expect(press("Enter", false, 13)).toBe(true);
  // 输入法用 Enter 确认候选词，那一下不是提交；两条线索各拦一种输入法。
  expect(press("Enter", true, 13)).toBe(false);
  expect(press("Enter", false, 229)).toBe(false);
  expect(press("Escape", false, 27)).toBe(false);
});
it("keeps keyboard focus inside the dialog and restores the opener after closing it", () => {
  function DialogExample() {
    const [open, setOpen] = useState(false);
    return <>
      <button onClick={() => setOpen(true)}>打开编辑</button>
      {open && <Modal title="编辑" onClose={() => setOpen(false)}><button>取消</button><button>保存</button></Modal>}
    </>;
  }
  render(<DialogExample />);
  const opener = screen.getByRole("button", { name: "打开编辑" });
  opener.focus();
  fireEvent.click(opener);
  const close = screen.getByRole("button", { name: "关闭弹窗" });
  fireEvent.keyDown(document, { key: "Tab" });
  expect(document.activeElement).toBe(close);
  fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
  expect(document.activeElement).toBe(screen.getByText("保存"));
  fireEvent.click(close);
  expect(screen.queryByRole("dialog", { name: "编辑" })).toBeNull();
  expect(document.activeElement).toBe(opener);
});
it("only closes the topmost dialog on Escape", () => {
  const outer = vi.fn(); const inner = vi.fn();
  render(<Modal title="外层" onClose={outer}><Modal title="内层" onClose={inner}>内容</Modal></Modal>);
  fireEvent.keyDown(document, { key: "Escape" });
  expect(inner).toHaveBeenCalledOnce();
  expect(outer).not.toHaveBeenCalled();
});

it("keeps a selector popup inside the dialog keyboard flow without closing its parent", () => {
  const close = vi.fn();
  render(<Modal title="配置" onClose={close}>
    <CustomSelect ariaLabel="选择" value="one" onChange={() => {}} options={[{ value: "one", label: "一" }, { value: "two", label: "二" }]} />
  </Modal>);
  const selector = screen.getByRole("button", { name: "选择：一" });
  fireEvent.click(selector);
  fireEvent.keyDown(screen.getByRole("option", { name: "一" }), { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(screen.getByRole("dialog", { name: "配置" })).toBeTruthy();
  expect(close).not.toHaveBeenCalled();
  fireEvent.click(selector);
  fireEvent.keyDown(screen.getByRole("option", { name: "一" }), { key: "Tab" });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "关闭弹窗" }));
});
