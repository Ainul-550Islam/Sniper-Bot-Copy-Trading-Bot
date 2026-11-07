"""Generated Playwright outputs must not change product release measurements."""

import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class ManifestArtifactContract(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="manifest-artifacts-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.write("release-manifest.json", json.dumps({"version": "0.1.0", "verification": {}}))
        self.write("crates/core/src/lib.rs", "#[test]\nfn example() {}\n")
        self.write("crates/core/migrations/0001_initial.sql", "SELECT 1;\n")
        self.write("docs/README.md", "# Fixture documentation\n")
        script = self.root / "scripts/update-release-manifest.sh"
        script.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/update-release-manifest.sh", script)
        self.baseline = self.measure()

    def write(self, relative, content):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def measure(self):
        subprocess.run(
            ["bash", str(self.root / "scripts/update-release-manifest.sh")],
            cwd=self.root,
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads((self.root / "release-manifest.json").read_text())["verification"]["manifest"]

    def test_playwright_test_results_do_not_change_product_measurement(self):
        self.write("apps/control-plane/test-results/.last-run.json", '{"status":"passed"}\n')
        self.write("apps/control-plane/test-results/run/trace.zip", "generated trace fixture")
        self.assertEqual(self.measure(), self.baseline)

    def test_playwright_reports_do_not_change_product_measurement(self):
        self.write("apps/control-plane/playwright-report/index.html", "generated report")
        self.write("apps/control-plane/blob-report/report.zip", "generated report fixture")
        self.assertEqual(self.measure(), self.baseline)

    def test_real_source_is_still_measured(self):
        self.write("crates/core/src/another.rs", "pub fn another() {}\n")
        measured = self.measure()
        self.assertEqual(measured["product_files"], self.baseline["product_files"] + 1)
        self.assertEqual(measured["rust_files"], self.baseline["rust_files"] + 1)
        self.assertNotEqual(measured["tree_digest"], self.baseline["tree_digest"])


if __name__ == "__main__":
    unittest.main()
