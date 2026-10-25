import { readdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { constants } from "node:os";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

async function discover(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await discover(path));
    else if (entry.isFile() && entry.name.endsWith(".test.mjs")) files.push(path);
  }
  return files.sort();
}

try {
  const files = await discover(join(root, "tests"));
  if (files.length === 0) throw new Error("No *.test.mjs files found in tests/");
  const child = spawn(process.execPath, ["--test", ...files], {
    cwd: root,
    stdio: "inherit",
    shell: false,
  });
  child.on("error", (error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
  child.on("exit", (code, signal) => {
    process.exitCode = code ?? (128 + (constants.signals[signal] ?? 1));
  });
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
