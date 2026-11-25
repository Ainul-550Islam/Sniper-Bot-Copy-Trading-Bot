#!/usr/bin/env python3
"""Every compose service that names CONFIG_PATH must be able to read it.

`docker-compose.yml` sets `CONFIG_PATH: /app/config.toml` inside the image for
the bot and for the one-shot migration service. `config.toml` is a *local*,
gitignored file (see .gitignore) created by `scripts/bootstrap-config.sh`, so
the path only exists inside a container if the service mounts it. The
migration service originally did not, which meant `docker compose up` reached
`db-migrate` with a config path that was not there.

This is a source-level contract check: it reads the compose file and asserts the
invariant. It does not run Docker, and it does not prove the stack starts.
"""

import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
COMPOSE = ROOT / "docker-compose.yml"


def services():
    return yaml.safe_load(COMPOSE.read_text(encoding="utf-8"))["services"]


def mounts(service):
    """Every volume spec of a service as a string (long form is normalised)."""
    out = []
    for vol in service.get("volumes") or []:
        if isinstance(vol, str):
            out.append(vol)
        elif isinstance(vol, dict):
            out.append(f"{vol.get('source')}:{vol.get('target')}")
    return out


class ComposeConfigMounts(unittest.TestCase):
    def test_every_config_path_service_mounts_a_config(self):
        offenders = []
        for name, service in services().items():
            env = service.get("environment") or {}
            config_path = env.get("CONFIG_PATH")
            if config_path is None:
                continue
            if not any(str(config_path) in vol for vol in mounts(service)):
                offenders.append(f"{name} sets CONFIG_PATH={config_path} but mounts no config")
        self.assertEqual(
            offenders,
            [],
            "a service that names CONFIG_PATH must mount the file there: "
            + "; ".join(offenders),
        )

    def test_the_config_mount_is_read_only(self):
        for name, service in services().items():
            for vol in mounts(service):
                if "config.toml" not in vol:
                    continue
                self.assertTrue(
                    vol.endswith(":ro"),
                    f"{name} must mount config.toml read-only, got {vol}",
                )

    def test_config_toml_is_ignored_and_the_example_is_not(self):
        gitignore = (ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()
        self.assertIn(
            "/config.toml",
            [line.strip() for line in gitignore],
            "the local config must stay gitignored",
        )
        self.assertIn(
            "!config.toml.example",
            [line.strip() for line in gitignore],
            "the shipped example must stay tracked",
        )
        self.assertTrue(
            (ROOT / "config.toml.example").is_file(),
            "config.toml.example is the tracked source an operator copies",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
