import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const cli = fileURLToPath(new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url));
const run = args => execFileSync(process.execPath, [cli, ...args], { stdio: "inherit" });

const root = fileURLToPath(new URL("../", import.meta.url));
const temp = await mkdtemp(join(tmpdir(), "noending-icons-"));
try {
  const source = join(temp, "app.svg");
  await writeFile(source, await readFile(join(root, "src/assets/noending-app-icon.svg"), "utf8"));
  const output = join(root, "src-tauri/icons");
  run(["icon", source, "--output", output]);
  run(["icon", source, "--output", output, "--png", "64"]);
} finally {
  await rm(temp, { recursive: true, force: true });
}
