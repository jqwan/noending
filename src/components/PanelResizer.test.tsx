import { useState } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import PanelResizer from "./PanelResizer";

const captureMethods = ["setPointerCapture", "hasPointerCapture", "releasePointerCapture"] as const;
const original = new Map(captureMethods.map(name => [name, Object.getOwnPropertyDescriptor(HTMLElement.prototype, name)]));
let captured = new WeakMap<HTMLElement, number>();
beforeEach(() => {
  captured = new WeakMap();
  vi.stubGlobal("PointerEvent", class extends MouseEvent {
    pointerId: number;
    isPrimary: boolean;
    constructor(type: string, init: MouseEventInit & { pointerId?: number; isPrimary?: boolean } = {}) {
      super(type, init); this.pointerId = init.pointerId ?? 1; this.isPrimary = init.isPrimary ?? true;
    }
  });
  Object.defineProperties(HTMLElement.prototype, {
    setPointerCapture: { configurable: true, value(this: HTMLElement, id: number) { captured.set(this, id); } },
    hasPointerCapture: { configurable: true, value(this: HTMLElement, id: number) { return captured.get(this) === id; } },
    releasePointerCapture: { configurable: true, value(this: HTMLElement) { captured.delete(this); } },
  });
});
afterEach(() => {
  cleanup(); vi.unstubAllGlobals();
  for (const name of captureMethods) {
    const descriptor = original.get(name);
    if (descriptor) Object.defineProperty(HTMLElement.prototype, name, descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, name);
  }
});
function Pane({ side = "left", label = "侧栏宽度" }: { side?: "left" | "right"; label?: string }) {
  const [width, setWidth] = useState(300);
  return <PanelResizer side={side} label={label} controls="pane" value={width} min={200} max={480} defaultValue={360} onResize={setWidth} />;
}
const start = (target: HTMLElement, pointerId = 1) => fireEvent.pointerDown(target, { pointerId, clientX: 300, button: 0 });
const move = (target: HTMLElement, clientX: number, pointerId = 1) => fireEvent.pointerMove(target, { pointerId, clientX });

it("resizes both edges in their physical direction and restores default width", () => {
  render(<><Pane label="左侧栏" /><Pane side="right" label="右侧栏" /></>);
  for (const [label, moved, boundary] of [["左侧栏", "340", "480"], ["右侧栏", "260", "200"]]) {
    const divider = screen.getByRole("separator", { name: label });
    start(divider); move(divider, 340);
    expect(divider.getAttribute("aria-valuenow")).toBe(moved);
    move(divider, 10_000);
    expect(divider.getAttribute("aria-valuenow")).toBe(boundary);
    fireEvent.pointerUp(divider, { pointerId: 1 });
    move(divider, -10_000);
    expect(divider.getAttribute("aria-valuenow")).toBe(boundary);
    fireEvent.doubleClick(divider);
    expect(divider.getAttribute("aria-valuenow")).toBe("360");
  }
});

it("releases drag ownership after cancellation, lost capture, blur or Escape", () => {
  render(<><Pane label="左侧栏" /><Pane side="right" label="右侧栏" /></>);
  const left = screen.getByRole("separator", { name: "左侧栏" });
  const right = screen.getByRole("separator", { name: "右侧栏" });
  const endings = {
    cancel: () => fireEvent.pointerCancel(left, { pointerId: 1 }),
    captureLoss: () => fireEvent.lostPointerCapture(left, { pointerId: 1 }),
    blur: () => fireEvent(window, new Event("blur")),
    escape: () => fireEvent.keyDown(left, { key: "Escape" }),
  };
  for (const [event, end] of Object.entries(endings)) {
    const leftWidth = Number(left.getAttribute("aria-valuenow"));
    const rightWidth = Number(right.getAttribute("aria-valuenow"));
    start(left); move(left, 320);
    start(right, 2); move(right, 280, 2);
    expect(right.getAttribute("aria-valuenow"), event).toBe(String(rightWidth));
    end();
    move(left, 400);
    expect(left.getAttribute("aria-valuenow"), event).toBe(String(leftWidth + 20));
    start(right, 2); move(right, 280, 2);
    expect(right.getAttribute("aria-valuenow"), event).toBe(String(rightWidth + 20));
    fireEvent.pointerUp(right, { pointerId: 2 });
  }
});

it("ignores secondary pointers and releases ownership when a dragging pane is removed", () => {
  const first = render(<Pane />);
  const divider = screen.getByRole("separator");
  fireEvent.pointerDown(divider, { pointerId: 1, clientX: 300, button: 2 });
  move(divider, 360);
  expect(divider.getAttribute("aria-valuenow")).toBe("300");
  fireEvent.pointerDown(divider, { pointerId: 2, clientX: 300, button: 0, isPrimary: false });
  move(divider, 360, 2);
  expect(divider.getAttribute("aria-valuenow")).toBe("300");
  start(divider); move(divider, 320);
  fireEvent.pointerUp(divider, { pointerId: 2 });
  move(divider, 340);
  expect(divider.getAttribute("aria-valuenow")).toBe("340");
  first.unmount();
  render(<Pane side="right" />);
  const next = screen.getByRole("separator");
  start(next); move(next, 280);
  expect(next.getAttribute("aria-valuenow")).toBe("320");
});
