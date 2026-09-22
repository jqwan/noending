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
  const mark = await readFile(join(root, "src/assets/noending-mark.svg"), "utf8");
  const source = join(temp, "app.svg");
  await writeFile(source, `<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
    <rect x="48" y="48" width="416" height="416" rx="96" fill="#f5f5f3"/>
    <g transform="translate(83.2 126.4) scale(3.6)">${mark.replace(/<\/?svg[^>]*>/g, "")}</g>
  </svg>`);
  const output = join(root, "src-tauri/icons");
  run(["icon", source, "--output", output]);
  run(["icon", source, "--output", output, "--png", "64"]);
} finally {
  await rm(temp, { recursive: true, force: true });
}
