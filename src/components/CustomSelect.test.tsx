import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import CustomSelect from "./CustomSelect";

describe("CustomSelect", () => {
  afterEach(cleanup);

  it("opens menu on click and selects an option", () => {
    const onChange = vi.fn();
    render(
      <CustomSelect
        value="all"
        onChange={onChange}
        ariaLabel="筛选"
        options={[
          { value: "all", label: "全部" },
          { value: "ok", label: "正常" },
          { value: "missing", label: "有目录缺失" },
        ]}
      />
    );

    const trigger = screen.getByRole("button", { name: "筛选：全部" });
    fireEvent.click(trigger);

    const menu = screen.getByRole("listbox", { name: "筛选" });
    expect(menu).toBeTruthy();

    const okOption = within(menu).getByRole("option", { name: "正常" });
    fireEvent.click(okOption);

    expect(onChange).toHaveBeenCalledWith("ok");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("closes dropdown on Escape key and outside click", () => {
    render(
      <div>
        <div data-testid="outside">外面</div>
        <CustomSelect
          value="recent"
          onChange={vi.fn()}
          ariaLabel="排序"
          options={[
            { value: "recent", label: "最近活动" },
            { value: "name", label: "名称" },
          ]}
        />
      </div>
    );

    const trigger = screen.getByRole("button", { name: "排序：最近活动" });
    fireEvent.click(trigger);
    expect(screen.getByRole("listbox")).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.click(trigger);
    expect(screen.getByRole("listbox")).toBeTruthy();

    fireEvent.mouseDown(screen.getByTestId("outside"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("navigates enabled options by keyboard and returns focus to the trigger", () => {
    const onChange = vi.fn();
    render(<CustomSelect value="first" ariaLabel="负责人" onChange={onChange} options={[
      { value: "first", label: "由我协调" },
      { value: "disabled", label: "不可用成员", disabled: true },
      { value: "second", label: "后端会话" },
    ]} />);
    const trigger = screen.getByRole("button", { name: "负责人：由我协调" });
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "由我协调" }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    const next = screen.getByRole("option", { name: "后端会话" });
    expect(document.activeElement).toBe(next);
    fireEvent.click(next);
    expect(onChange).toHaveBeenCalledWith("second");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    fireEvent.click(trigger);
    fireEvent.keyDown(screen.getByRole("option", { name: "由我协调" }), { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("keeps a disabled selector closed and ignores disabled options", () => {
    const onChange = vi.fn();
    const props = { value: "first", ariaLabel: "筛选", onChange, options: [
      { value: "first", label: "全部" }, { value: "second", label: "不可用", disabled: true },
    ] };
    const view = render(<CustomSelect {...props} disabled />);
    fireEvent.click(screen.getByRole("button", { name: "筛选：全部" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    view.rerender(<CustomSelect {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "筛选：全部" }));
    fireEvent.click(screen.getByRole("option", { name: "不可用" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("listbox")).toBeTruthy();
    view.rerender(<CustomSelect {...props} disabled />);
    expect(screen.queryByRole("listbox")).toBeNull();
    view.rerender(<CustomSelect {...props} />);
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
