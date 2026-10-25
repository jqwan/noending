import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import PageHeader from "./PageHeader";

describe("PageHeader", () => {
  it("keeps the title out of the window drag region", () => {
    const { container } = render(<PageHeader title="新建会话" />);

    // 这条 band 吸在应用标题栏上，自己得是可拖的：tauri 的 drag.js 只认这个属性。
    expect(container.querySelector(".page-head")?.getAttribute("data-tauri-drag-region")).toBe("deep");
    // 标题是内容不是窗口边框。少了 ="false"，按一下标题就把整个窗口带走，
    // 双击还会最大化——tauri 沿 composed path 上溯，遇到 ="false" 就停。
    expect(container.querySelector(".page-title")?.getAttribute("data-tauri-drag-region")).toBe("false");
  });
});
