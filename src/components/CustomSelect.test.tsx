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
            { value: "recent", label: "最近活动 ↓" },
            { value: "name", label: "名称" },
          ]}
        />
      </div>
    );

    const trigger = screen.getByRole("button", { name: "排序：最近活动 ↓" });
    fireEvent.click(trigger);
    expect(screen.getByRole("listbox")).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.click(trigger);
    expect(screen.getByRole("listbox")).toBeTruthy();

    fireEvent.mouseDown(screen.getByTestId("outside"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
