import { expect, it } from "vitest";
import { displayPath } from "./pathDisplay";

it.each([
  [String.raw`C:\Workspace\project/.claude/skills`, String.raw`C:\Workspace\project\.claude\skills`],
  ["c:/Users/张三/skills", String.raw`c:\Users\张三\skills`],
  [String.raw`\\?\C:\Users\me/skills/SKILL.md`, String.raw`C:\Users\me\skills\SKILL.md`],
  ["//?/C:/Users/me/skills", String.raw`C:\Users\me\skills`],
  [String.raw`\\?\UNC\server\share/skills`, String.raw`\\server\share\skills`],
  ["//?/UNC/server/share/skills", String.raw`\\server\share\skills`],
  [String.raw`\\server\share/skills`, String.raw`\\server\share\skills`],
  ["//server/share/skills", String.raw`\\server\share\skills`],
  ["/Users/me/.claude/skills", "/Users/me/.claude/skills"],
  [String.raw`/Users/me/back\slash/skills`, String.raw`/Users/me/back\slash/skills`],
  ["https://example.com/mcp", "https://example.com/mcp"],
  [String.raw`\\?\Volume{123}\skills`, String.raw`\\?\Volume{123}\skills`],
  ["", ""],
])("displays %s as %s", (input, expected) => {
  expect(displayPath(input)).toBe(expected);
});
