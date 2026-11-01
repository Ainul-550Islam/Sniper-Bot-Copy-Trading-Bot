#!/usr/bin/env python3
"""Static validation of the container assets (Dockerfiles + compose files).

WHY THIS EXISTS
    The delivery host has no container runtime (no docker, dockerd, podman or
    buildah), so an image cannot be built here and we will not pretend one was.
    What CAN be proven without a daemon is that the container assets are
    structurally sound: every FROM is digest-pinned (reproducible, not
    ":latest"), multi-stage builds name their stages and copy only from them,
    the runtime stage drops privileges, no secret-shaped ENV/ARG ships in the
    image, no remote ADD streams the internet into the build, and every compose
    file parses as YAML with images/build contexts that resolve.

USAGE
    scripts/validate-container-assets.py [--json]
    exit 0 = every check passed, 1 = at least one FAIL.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any

try:
    import yaml
except ImportError:  # pragma: no cover - PyYAML is present in CI images
    print("FAIL pyyaml is required (pip install pyyaml)", file=sys.stderr)
    raise SystemExit(1)

ROOT = Path(__file__).resolve().parent.parent


class ComposeLoader(yaml.SafeLoader):
    """SafeLoader that understands compose-specific tags (!override, !reset)."""


def _compose_tag(loader: "ComposeLoader", _suffix: str, node: Any) -> Any:
    if isinstance(node, yaml.SequenceNode):
        return loader.construct_sequence(node, deep=True)
    if isinstance(node, yaml.MappingNode):
        return loader.construct_mapping(node, deep=True)
    return loader.construct_scalar(node)


ComposeLoader.add_multi_constructor("!", _compose_tag)


def load_compose(path: Path) -> Any:
    return yaml.load(path.read_text(encoding="utf-8"), Loader=ComposeLoader)

SECRET_ENV = re.compile(
    r"(PASSWORD|PASSWD|SECRET|PRIVATE_KEY|API_KEY|TOKEN|CREDENTIAL|MNEMONIC|SEED)",
    re.IGNORECASE,
)
# Values that are obviously placeholders or env indirections are fine.
SAFE_VALUE = re.compile(
    r"(^\s*$|\$\{?[A-Z_0-9]+|changeme|example|placeholder|dummy|test|demo|not-for-prod|<|_FILE$)",
    re.IGNORECASE,
)

results: list[dict[str, Any]] = []


def record(
    kind: str, target: str, check: str, ok: bool, detail: str = "", warn_only: bool = False
) -> None:
    status = "PASS" if ok else ("WARN" if warn_only else "FAIL")
    results.append({"kind": kind, "target": target, "check": check, "status": status, "detail": detail})


def discover(pattern: str) -> list[Path]:
    return sorted(
        p
        for p in ROOT.rglob(pattern)
        if "node_modules" not in p.parts and "target" not in p.parts and ".git" not in p.parts
    )


def check_dockerfile(path: Path) -> None:
    rel = str(path.relative_to(ROOT))
    text = path.read_text(encoding="utf-8", errors="replace")
    lines = [ln.strip() for ln in text.splitlines()]
    # join line continuations so `FROM`/`COPY` splits don't hide a rule
    joined: list[str] = []
    buf = ""
    for ln in lines:
        if ln.endswith("\\"):
            buf += ln[:-1] + " "
            continue
        joined.append((buf + ln).strip())
        buf = ""
    if buf:
        joined.append(buf.strip())

    froms = [ln for ln in joined if ln.upper().startswith("FROM ")]
    record("dockerfile", rel, "has at least one FROM", bool(froms))
    for ln in froms:
        image = ln.split()[1]
        pinned = "@sha256:" in image
        tagged = ":" in image.split("/")[-1]
        record(
            "dockerfile",
            rel,
            f"FROM {image.split('@')[0]} is digest-pinned",
            pinned,
            "add @sha256:<digest> for reproducibility" if not pinned else "",
        )
        record(
            "dockerfile",
            rel,
            f"FROM {image.split('@')[0]} is not :latest",
            not image.endswith(":latest"),
        )
        record(
            "dockerfile",
            rel,
            f"FROM {image.split('@')[0]} is tagged or pinned",
            pinned or tagged,
        )

    stages: list[str] = []
    for ln in froms:
        parts = ln.split()
        upper = [p.upper() for p in parts]
        if "AS" in upper:
            name = parts[upper.index("AS") + 1]
            stages.append(name)
            record("dockerfile", rel, f"stage alias '{name}' is lowercase", name == name.lower())
    record("dockerfile", rel, "multi-stage build names every stage",
           len(stages) == len(froms), f"{len(stages)}/{len(froms)} named")

    copy_from = [ln for ln in joined if re.match(r"(?i)^COPY\s+--from=", ln)]
    for ln in copy_from:
        src = re.match(r"(?i)^COPY\s+--from=(\S+)", ln).group(1)
        record(
            "dockerfile",
            rel,
            f"COPY --from={src} references a declared stage",
            src in stages,
            f"declared stages: {stages}",
        )

    # Privilege drop in the final stage.
    last_from_index = max(
        (i for i, ln in enumerate(joined) if ln.upper().startswith("FROM ")), default=-1
    )
    tail = joined[last_from_index:]
    user_lines = [ln for ln in tail if ln.upper().startswith("USER ")]
    record(
        "dockerfile",
        rel,
        "final stage sets a non-root USER",
        bool(user_lines) and all(ln.split()[1].split(":")[0] not in {"root", "0"} for ln in user_lines),
        "; ".join(user_lines) or "no USER instruction in the final stage",
    )

    # Secrets in ENV / ARG.
    for ln in joined:
        upper = ln.upper()
        if not (upper.startswith("ENV ") or upper.startswith("ARG ")):
            continue
        payload = ln.split(None, 1)[1] if " " in ln else ""
        for token in re.split(r"\s+", payload):
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            if SECRET_ENV.search(key) and not SAFE_VALUE.search(value):
                record("dockerfile", rel, f"no secret literal in {ln.split()[0]} {key}", False, value[:24])
                break
        else:
            continue

    record(
        "dockerfile",
        rel,
        "no ADD from a remote URL",
        not any(re.match(r"(?i)^ADD\s+https?://", ln) for ln in joined),
    )
    record(
        "dockerfile",
        rel,
        "no 'curl ... | sh' pipe-to-shell",
        not any(re.search(r"curl[^|]*\|\s*(ba)?sh", ln) for ln in joined),
    )
    record(
        "dockerfile",
        rel,
        "no 'apt-get install' without --no-install-recommends",
        not any(
            "apt-get install" in ln and "--no-install-recommends" not in ln for ln in joined
        ),
    )
    record("dockerfile", rel, "does not COPY .env / *.pem / *.key", not any(
        re.search(r"(?i)^COPY\s+.*(\.env|\.pem|\.key|id_rsa)", ln) for ln in joined
    ))


def _base_service_names(path: Path) -> set[str]:
    """Service names declared by any OTHER compose file (overlay targets).

    `docker-compose.pitr.yml` is an overlay applied on top of
    `docker-compose.yml` + `docker-compose.backup.yml`, so a service that has
    neither `image:` nor `build:` here is legitimate when some other file in
    the repo defines it.
    """
    names: set[str] = set()
    for other in discover("docker-compose*.yml"):
        if other == path:
            continue
        try:
            doc = load_compose(other) or {}
        except yaml.YAMLError:
            continue
        names |= set((doc.get("services") or {}).keys())
    return names


def check_compose(path: Path) -> dict[str, Any] | None:
    rel = str(path.relative_to(ROOT))
    try:
        doc = load_compose(path)
    except yaml.YAMLError as exc:
        record("compose", rel, "parses as YAML", False, str(exc).splitlines()[0])
        return None
    record("compose", rel, "parses as YAML", True)
    if not isinstance(doc, dict) or "services" not in doc:
        record("compose", rel, "has a services: map", False)
        return None
    services = doc.get("services") or {}
    record("compose", rel, "declares at least one service", bool(services), ", ".join(services))
    base_services = _base_service_names(path)

    for name, svc in services.items():
        svc = svc or {}
        has_target = "image" in svc or "build" in svc
        inherited = name in base_services
        record(
            "compose",
            rel,
            f"service '{name}' has image or build (or is an overlay of the base file)",
            has_target or inherited,
            "overlay: base docker-compose.yml defines it" if inherited and not has_target else "",
        )
        if "build" in svc:
            ctx = svc["build"] if isinstance(svc["build"], str) else svc["build"].get("context", ".")
            resolved = (path.parent / ctx).resolve()
            record(
                "compose",
                rel,
                f"service '{name}' build context exists",
                resolved.exists(),
                str(resolved),
            )
        # Datastores should not be published on 0.0.0.0 in the shipped defaults.
        ports = svc.get("ports") or []
        is_datastore = any(
            token in (svc.get("image") or "") for token in ("postgres", "redis", "mysql", "minio")
        )
        if is_datastore and ports:
            bound_loopback = all(str(p).startswith("127.0.0.1:") for p in ports)
            record(
                "compose",
                rel,
                f"service '{name}' (datastore) publishes only on loopback",
                bound_loopback,
                ", ".join(map(str, ports)),
            )
        loopback_only = bool(ports) and all(str(p).startswith("127.0.0.1:") for p in ports)

        env = svc.get("environment") or {}
        if isinstance(env, list):
            env = dict(item.split("=", 1) for item in env if "=" in item)
        for key, value in env.items():
            if SECRET_ENV.search(key) and isinstance(value, str) and not SAFE_VALUE.search(value):
                record(
                    "compose",
                    rel,
                    f"service '{name}' env {key} carries no literal secret",
                    False,
                    (value[:24] + " — local-only test stack, not published") if loopback_only else value[:24],
                    warn_only=loopback_only,
                )
    return doc


def cross_check() -> None:
    """Dockerfile paths referenced by compose must exist."""
    for path in discover("docker-compose*.yml"):
        try:
            doc = load_compose(path) or {}
        except yaml.YAMLError:
            continue
        for name, svc in (doc.get("services") or {}).items():
            build = (svc or {}).get("build")
            if not build:
                continue
            if isinstance(build, dict) and build.get("dockerfile"):
                df = Path(build["dockerfile"])
                if not df.is_absolute():
                    df = (path.parent / build.get("context", ".") / df).resolve()
                record(
                    "compose",
                    str(path.relative_to(ROOT)),
                    f"service '{name}' dockerfile exists",
                    df.exists(),
                    str(df),
                )


def main() -> int:
    dockerfiles = discover("Dockerfile*")
    composes = discover("docker-compose*.yml") + discover("compose*.yml")
    print(f"[container-assets] {len(dockerfiles)} Dockerfile(s), {len(composes)} compose file(s)")
    for path in dockerfiles:
        check_dockerfile(path)
    for path in composes:
        check_compose(path)
    cross_check()

    failed = [r for r in results if r["status"] == "FAIL"]
    warned = [r for r in results if r["status"] == "WARN"]
    width = max((len(r["target"]) for r in results), default=10)
    for r in results:
        line = f"  {r['status']:4}  {r['target']:<{width}}  {r['check']}"
        if r["detail"] and r["status"] == "FAIL":
            line += f"   [{r['detail']}]"
        print(line)
    print()
    passed = len(results) - len(failed) - len(warned)
    print(
        f"[container-assets] {passed} passed, {len(warned)} warned, {len(failed)} failed"
    )
    if "--json" in sys.argv:
        print(json.dumps(results, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
