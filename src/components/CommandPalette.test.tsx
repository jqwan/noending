import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import CommandPalette from "./CommandPalette";

vi.mock("../api", () => ({
  api: {
    listWorkstreams: vi.fn().mockResolvedValue([]),
    listSessions: vi.fn().mockResolvedValue([]),
    search: vi.fn().mockResolvedValue([]),
  },
}));

afterEach(cleanup);

it("opens the new-session page and closes the palette", async () => {
  const navigate = vi.fn();
  const onClose = vi.fn();
  await act(async () => {
    render(<CommandPalette navigate={navigate} onClose={onClose} />);
  });

  expect(screen.queryByText("前往首页")).toBeNull();
  fireEvent.click(screen.getByText("新建会话"));
  expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  expect(onClose).toHaveBeenCalledTimes(1);
});
