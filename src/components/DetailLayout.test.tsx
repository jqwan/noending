import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import PageHeader from "../layout/PageHeader";
import { viewState } from "../hooks/useViewState";
import { DetailLayout, DetailPanelProvider } from "./DetailLayout";
import CustomSelect from "./CustomSelect";

beforeEach(() => { viewState.clear(); localStorage.clear(); });
afterEach(() => { cleanup(); viewState.clear(); localStorage.clear(); });
function Page({ kind = "session" }: { kind?: string }) {
  return <DetailPanelProvider kind={kind}><PageHeader title="详情" navigation={<button>任务详情</button>} actions={<button>编辑</button>} />
    <DetailLayout panel={<><input aria-label="右栏草稿" /><CustomSelect ariaLabel="选择" value="one" onChange={() => {}}
      options={[{ value: "one", label: "一" }, { value: "two", label: "二" }]} /></>}>
      <input aria-label="正文草稿" />
    </DetailLayout>
  </DetailPanelProvider>;
}

it("closes from Escape, restores focus and preserves both panes' input", () => {
  render(<Page />);
  const side = screen.getByRole("textbox", { name: "右栏草稿" });
  fireEvent.change(side, { target: { value: "保留右栏草稿" } });
  fireEvent.change(screen.getByRole("textbox", { name: "正文草稿" }), { target: { value: "保留正文草稿" } });
  side.focus(); fireEvent.keyDown(side, { key: "Escape" });
  const toggle = screen.getByRole("button", { name: "展开详情栏" });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(document.getElementById(toggle.getAttribute("aria-controls")!)?.hidden).toBe(true);
  expect(document.activeElement).toBe(toggle);
  expect(screen.queryByRole("complementary", { name: "详情栏" })).toBeNull();
  expect((screen.getByRole("textbox", { name: "正文草稿" }) as HTMLInputElement).value).toBe("保留正文草稿");
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(document.getElementById(toggle.getAttribute("aria-controls")!)?.hidden).toBe(false);
  expect((screen.getByRole("textbox", { name: "右栏草稿" }) as HTMLInputElement).value).toBe("保留右栏草稿");
});

it("remembers panel visibility after remount while keeping page types independent", () => {
  const view = render(<Page />);
  fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
  view.unmount(); viewState.clear();
  const session = render(<Page />);
  screen.getByRole("button", { name: "展开详情栏" });
  session.unmount();
  render(<Page kind="project" />);
  screen.getByRole("button", { name: "收起详情栏" });
  expect(screen.getByRole("complementary", { name: "详情栏" })).toBeTruthy();
});

it.each([["task", "collaboration", "session"], ["session", "terminal", "task"]])(
  "shares panel width and visibility between %s and %s while keeping %s independent",
  (detail, subpage, other) => {
    const page = render(<Page kind={detail} />);
    fireEvent.keyDown(screen.getByRole("separator", { name: "详情栏宽度" }), { key: "End" });
    fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
    page.rerender(<Page kind={subpage} />);
    fireEvent.click(screen.getByRole("button", { name: "展开详情栏" }));
    expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("480");
    fireEvent.keyDown(screen.getByRole("separator", { name: "详情栏宽度" }), { key: "Home" });
    page.unmount(); viewState.clear();

    // 清空内存后从持久化偏好恢复，另一子页的修改也会反向同步。
    const restored = render(<Page kind={detail} />);
    expect(screen.getByRole("complementary", { name: "详情栏" })).toBeTruthy();
    expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("280");
    restored.rerender(<Page kind={other} />);
    expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("360");
    restored.unmount(); viewState.clear();
    render(<Page kind={subpage} />);
    expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("280");
  },
);

it("lets a selector consume Escape without hiding the detail panel", () => {
  render(<Page />);
  fireEvent.click(screen.getByRole("button", { name: "选择：一" }));
  fireEvent.keyDown(screen.getByRole("option", { name: "一" }), { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(screen.getByRole("complementary", { name: "详情栏" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "选择：一" })).toBe(document.activeElement);
});

it("resizes within bounds, remembers each page type's width and restores the default on double click", () => {
  const first = render(<Page />);
  const resizer = screen.getByRole("separator", { name: "详情栏宽度" });
  expect(resizer.getAttribute("aria-valuenow")).toBe("360");
  fireEvent.keyDown(resizer, { key: "ArrowLeft" });
  expect(resizer.getAttribute("aria-valuenow")).toBe("368");
  fireEvent.keyDown(resizer, { key: "ArrowRight" });
  expect(resizer.getAttribute("aria-valuenow")).toBe("360");
  fireEvent.keyDown(resizer, { key: "Home" });
  fireEvent.keyDown(resizer, { key: "ArrowRight" });
  expect(resizer.getAttribute("aria-valuenow")).toBe("280");
  fireEvent.keyDown(resizer, { key: "End" });
  fireEvent.keyDown(resizer, { key: "ArrowLeft" });
  expect(resizer.getAttribute("aria-valuenow")).toBe("480");
  fireEvent.click(screen.getByRole("button", { name: "收起详情栏" }));
  expect(screen.queryByRole("separator", { name: "详情栏宽度" })).toBeNull();
  first.unmount(); viewState.clear();
  const restored = render(<Page />);
  fireEvent.click(screen.getByRole("button", { name: "展开详情栏" }));
  expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("480");
  restored.unmount(); viewState.clear();
  const project = render(<Page kind="project" />);
  expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("360");
  project.unmount();
  render(<Page />);
  fireEvent.doubleClick(screen.getByRole("separator", { name: "详情栏宽度" }));
  expect(screen.getByRole("separator", { name: "详情栏宽度" }).getAttribute("aria-valuenow")).toBe("360");
});
