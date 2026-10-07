import { expect, it } from "vitest";
import { httpsRemoteUrl } from "./remoteUrl";

it("normalizes remote spellings to openable https URLs", () => {
  // scp 风格（克隆走 SSH 的常见形态）。
  expect(httpsRemoteUrl("git@github.com:user/repo.git")).toBe(
    "https://github.com/user/repo.git",
  );
  // ssh URL，带不带用户名、带不带端口都能归一。
  expect(httpsRemoteUrl("ssh://git@host.example.com/path/repo.git")).toBe(
    "https://host.example.com/path/repo.git",
  );
  expect(httpsRemoteUrl("ssh://host.example.com:2222/path/repo.git")).toBe(
    "https://host.example.com/path/repo.git",
  );
  // git 守护进程协议。
  expect(httpsRemoteUrl("git://host.example.com/repo.git")).toBe(
    "https://host.example.com/repo.git",
  );
  // http(s) 原样——改写反而可能造出打不开的链接。
  expect(httpsRemoteUrl("https://example.com/me/repo.git")).toBe(
    "https://example.com/me/repo.git",
  );
  expect(httpsRemoteUrl("http://example.com/repo")).toBe(
    "http://example.com/repo",
  );
  // 本地路径不碰：那是“远程指向本机目录”的合法配置，点击由后端兜底拒绝。
  expect(httpsRemoteUrl("/srv/git/repo")).toBe("/srv/git/repo");
  expect(httpsRemoteUrl("C:\\repos\\repo")).toBe("C:\\repos\\repo");
  expect(httpsRemoteUrl("./sibling/repo")).toBe("./sibling/repo");
  // 前后空白不影响归一。
  expect(httpsRemoteUrl("  git@github.com:user/repo.git  ")).toBe(
    "https://github.com/user/repo.git",
  );
});
