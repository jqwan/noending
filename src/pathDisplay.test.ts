import { expect, it } from "vitest";
import { displayPath } from "./pathDisplay";

it("renders Windows paths with native separators and removes display-only prefixes", () => {
  const windowsPaths = [
  [String.raw`C:\Workspace\project/.claude/skills`, String.raw`C:\Workspace\project\.claude\skills`],
  ["c:/Users/张三/skills", String.raw`c:\Users\张三\skills`],
  [String.raw`\\?\C:\Users\me/skills/SKILL.md`, String.raw`C:\Users\me\skills\SKILL.md`],
  [String.raw`\\?\UNC\server\share/skills`, String.raw`\\server\share\skills`],
  [String.raw`\\server\share/skills`, String.raw`\\server\share\skills`],
  ];
  for (const [input, expected] of windowsPaths) expect(displayPath(input), input).toBe(expected);
});

it("preserves paths that do not use a recognized Windows spelling", () => {
  const paths = [
    "/Users/me/.claude/skills",
    String.raw`/Users/me/back\slash/skills`,
    "https://example.com/mcp",
    String.raw`\\?\Volume{123}\skills`,
    "",
  ];
  for (const input of paths) expect(displayPath(input), input).toBe(input);
});
