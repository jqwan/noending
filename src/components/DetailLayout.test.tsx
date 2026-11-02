import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import PageHeader from "../layout/PageHeader";
import { viewState } from "../hooks/useViewState";
import { DetailLayout, DetailPanelProvider } from "./DetailLayout";
import CustomSelect from "./CustomSelect";

beforeEach(() => { viewState.clear(); localStorage.clear(); });
afterEach(() => { cleanup(); viewState.clear(); localStorage.clear(); });
function Page({ kind = "session" }: { kind?: string }) {
  return <DetailPanelProvider kind={kind}><PageHeader title="详情" />
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
  expect(document.activeElement).toBe(toggle);
  expect(screen.queryByRole("complementary", { name: "详情栏" })).toBeNull();
  expect((screen.getByRole("textbox", { name: "正文草稿" }) as HTMLInputElement).value).toBe("保留正文草稿");
  fireEvent.click(toggle);
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

it("lets a selector consume Escape without hiding the detail panel", () => {
  render(<Page />);
  fireEvent.click(screen.getByRole("button", { name: "选择：一" }));
  fireEvent.keyDown(screen.getByRole("option", { name: "一" }), { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(screen.getByRole("complementary", { name: "详情栏" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "选择：一" })).toBe(document.activeElement);
});
