import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const runner = join(dirname(fileURLToPath(import.meta.url)), "../scripts/run-node-tests.mjs");

function fixture(run) {
  const root = mkdtempSync(join(tmpdir(), "node tests "));
  try {
    mkdirSync(join(root, "scripts"));
    mkdirSync(join(root, "tests"));
    const script = join(root, "scripts/run-node-tests.mjs");
    copyFileSync(runner, script);
    // A fresh process must not inherit node:test's internal child-runner flag.
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    run(root, () => spawnSync(process.execPath, [script], { encoding: "utf8", env }));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("test runner recursively discovers files and handles spaces in paths", () => {
  fixture((root, run) => {
    mkdirSync(join(root, "tests/nested space"));
    writeFileSync(join(root, "tests/nested space/passing.test.mjs"),
      'import { test } from "node:test"; test("discovered", () => {});\n');
    writeFileSync(join(root, "tests/ignored.mjs"), 'throw new Error("not a test");\n');
    const result = run();
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /discovered/);
  });
});

test("test runner fails instead of silently passing an empty test directory", () => {
  fixture((_root, run) => {
    const result = run();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /No \*\.test\.mjs files/);
  });
});

test("test runner preserves a failing test exit status", () => {
  fixture((root, run) => {
    writeFileSync(join(root, "tests/failing.test.mjs"),
      'import { test } from "node:test"; test("fails", () => { throw new Error("intentional failure"); });\n');
    const result = run();
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /intentional failure/);
  });
});
