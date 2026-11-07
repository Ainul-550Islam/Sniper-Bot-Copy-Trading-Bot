#!/usr/bin/env python3
"""Keep CI failure propagation and the repository's Rust pin explicit."""

from pathlib import Path
import unittest
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


class CiFailureContract(unittest.TestCase):
    def test_external_registry_tests_preserve_failure_status(self):
        text = (ROOT / ".github/workflows/ci.yml").read_text()
        job = text.split("  external-gated:\n", 1)[1]
        command = next(line.strip()[5:] for line in job.splitlines() if line.strip().startswith("run: cargo test"))
        self.assertNotIn("||", command, "a failed registry test must fail the job, not print a success-path message")
        self.assertIn("--locked", command)
        self.assertIn("-p sniper-suite", command)

    def test_release_jobs_do_not_silently_select_a_different_rust(self):
        text = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertFalse("uses: dtolnay/rust-toolchain@stable" in text, "CI must use the pinned Rust release, not rolling stable")
        pin = (ROOT / "rust-toolchain.toml").read_text().split('channel = "', 1)[1].split('"', 1)[0]
        for job_name in ["release", "external-gated"]:
            job = text.split(f"  {job_name}:\n", 1)[1].split("\n  #", 1)[0]
            self.assertIn(f"uses: dtolnay/rust-toolchain@{pin}", job)


    def run_truthfulness_fixture(self, missing_target=False, banned_literal=False):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / "scripts/forbid-fake-data.sh"
            script.parent.mkdir(parents=True)
            script.write_bytes((ROOT / "scripts/forbid-fake-data.sh").read_bytes())
            targets = ["crates/server/src/saas", "crates/server/src/trading_data_plane",
                       "apps/control-plane/src/app", "apps/control-plane/src/components",
                       "apps/control-plane/src/lib"]
            for target in targets[:-1] if missing_target else targets:
                directory = root / target
                directory.mkdir(parents=True)
                (directory / "source.txt").write_text("ordinary source text\n")
            if banned_literal:
                (root / targets[0] / "source.txt").write_text("acme-quant\n")
            return subprocess.run(["bash", str(script)], cwd=root, capture_output=True, text=True)

    def test_truthfulness_gate_refuses_an_unscannable_production_target(self):
        result = self.run_truthfulness_fixture(missing_target=True)
        self.assertNotEqual(result.returncode, 0,
                            "a missing production target must not be reported as a clean scan")

    def test_truthfulness_gate_accepts_a_fully_scanned_clean_tree(self):
        result = self.run_truthfulness_fixture()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_truthfulness_gate_still_rejects_a_forbidden_literal(self):
        result = self.run_truthfulness_fixture(banned_literal=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
