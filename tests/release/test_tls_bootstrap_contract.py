#!/usr/bin/env python3
"""Execute TLS bootstrap against command shims; no Docker or ACME side effects."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class TlsBootstrapContract(unittest.TestCase):
    def test_one_shot_certificate_issuance_overrides_the_renewal_entrypoint(self):
        with tempfile.TemporaryDirectory(prefix="tls-bootstrap-contract-") as directory:
            fixture = Path(directory)
            (fixture / "scripts").mkdir()
            (fixture / "deploy/nginx").mkdir(parents=True)
            (fixture / "deploy/nginx/dhparam.pem").write_text("test fixture, never a deployed key\n")
            script = fixture / "scripts/bootstrap-tls.sh"
            shutil.copyfile(ROOT / "scripts/bootstrap-tls.sh", script)
            binary = fixture / "bin"
            binary.mkdir()
            docker = binary / "docker"
            docker.write_text(f"#!{sys.executable}\n" + r'''
import json, os, sys
args = sys.argv[1:]
with open(os.environ["MOCK_DOCKER_LOG"], "a") as log:
    log.write(json.dumps(args) + "\n")
if "certonly" in args:
    # The Compose service has an explicit infinite renewal-loop entrypoint.
    # Passing a command does NOT override it: require the actual CLI override.
    if "--entrypoint" not in args or args[args.index("--entrypoint") + 1] != "certbot":
        print("certonly would inherit the renewal loop instead of issuing a certificate", file=sys.stderr)
        sys.exit(65)
''')
            docker.chmod(0o755)
            curl = binary / "curl"
            curl.write_text("#!/bin/sh\nexit 0\n")
            curl.chmod(0o755)
            log = fixture / "docker.jsonl"
            environment = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"],
                               MOCK_DOCKER_LOG=str(log), SNIPER_PUBLIC_HOST="example.invalid",
                               SNIPER_ACME_EMAIL="operator@example.invalid")
            result = subprocess.run(["bash", str(script)], cwd=fixture, env=environment,
                                    text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            issuance = [call for call in calls if "certonly" in call]
            self.assertEqual(len(issuance), 1, "bootstrap must still request a certificate")
            self.assertIn("--entrypoint", issuance[0])
            self.assertLess(next(i for i, call in enumerate(calls) if "up" in call), calls.index(issuance[0]))
            self.assertEqual(calls[-1][-4:], ["nginx", "nginx", "-s", "reload"])


if __name__ == "__main__":
    unittest.main()
