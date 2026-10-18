import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import PreferenceList from "./PreferenceList";
vi.mock("../../api", () => ({ api: { listPreferences: vi.fn(), promotePreference: vi.fn() } }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
beforeEach(() => { vi.resetAllMocks(); });
afterEach(cleanup);
it("promotes a locked project preference and refreshes both project and user lists", async () => {
  let promoted = false;
  const item = { id: "pref", project_id: "p", content: "保持简洁", locked: true };
  vi.mocked(api.listPreferences).mockImplementation(async scope => scope ? (promoted ? [] : [item]) : (promoted ? [{ ...item, project_id: null }] : []));
  vi.mocked(api.promotePreference).mockImplementation(async () => { promoted = true; return { ...item, project_id: null }; });
  render(<><PreferenceList projectId="p" /><PreferenceList projectId={null} /></>);
  fireEvent.click(await screen.findByRole("button", { name: "升级为用户偏好：保持简洁" }));
  await waitFor(() => expect(api.promotePreference).toHaveBeenCalledWith("pref"));
  await waitFor(() => expect(screen.queryByRole("button", { name: "升级为用户偏好：保持简洁" })).toBeNull());
  const user = screen.getByRole("region", { name: "用户偏好" });
  expect(user.textContent).toContain("保持简洁");
  expect(user.textContent).toContain("已锁定");
});

it("opens the add preference modal when the add icon button is clicked", async () => {
  vi.mocked(api.listPreferences).mockResolvedValue([]);
  render(<PreferenceList projectId="proj-1" />);
  const addBtn = await screen.findByRole("button", { name: "新增偏好" });
  expect(addBtn.title).toBe("新增偏好");
  fireEvent.click(addBtn);
  expect(screen.getByRole("dialog", { name: "新增偏好" })).toBeDefined();
});

