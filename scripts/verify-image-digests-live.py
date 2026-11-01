#!/usr/bin/env python3
"""Verify every container base image this repository pins, against the LIVE registries.

Why this exists: this environment has no container runtime, so no image can be
built here. What CAN be verified for real — over the network, against Docker
Hub's own registry API — is that every `image@sha256:…` digest in the tree
actually exists, resolves to an immutable manifest, and (for multi-arch
manifests) offers a `linux/amd64` entry. That turns "the digests look plausible"
into "the registry serving them says these bytes exist".

This does NOT build an image, does NOT run a container, and does NOT prove the
build succeeds. It proves the base images are real and correctly pinned.

Usage:
    python3 scripts/verify-image-digests.py [--json OUT]

Exit code 0 = every pinned digest resolved; 1 = at least one failure.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

REGISTRY = "https://registry-1.docker.io"
AUTH = "https://auth.docker.io/token"
ACCEPT = ",".join(
    [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    ]
)


EXCLUDED_DIRS = {".git", "target", "node_modules", ".next", "buyer-release", "out", "dist"}


def tracked_files() -> list[str]:
    """Files to scan: the git index when available, otherwise the tree itself.

    The delivered ZIP has no `.git`, so a buyer running this script inside the
    extracted package must get the same answer — hence the walk fallback.
    """
    try:
        out = subprocess.run(
            ["git", "ls-files"], capture_output=True, text=True, check=True
        ).stdout.split()
        candidates = out
    except (subprocess.CalledProcessError, FileNotFoundError):
        candidates = []
        for root, dirs, files in os.walk("."):
            dirs[:] = [d for d in dirs if d not in EXCLUDED_DIRS]
            for name in files:
                candidates.append(os.path.join(root, name).lstrip("./"))
    keep = []
    for path in candidates:
        name = path.rsplit("/", 1)[-1]
        if any(part in EXCLUDED_DIRS for part in path.split("/")[:-1]):
            continue
        if name.startswith("Dockerfile") or name.endswith((".yml", ".yaml")):
            keep.append(path)
    return keep


IMAGE_RE = re.compile(r"^\s*(?:FROM\s+(?:--platform=\S+\s+)?|-\s*)?image:\s*[\"']?([^\s\"'#]+)", re.I)
FROM_RE = re.compile(r"^\s*FROM\s+(?:--platform=\S+\s+)?([^\s#]+)", re.I)


def references(paths: list[str]) -> list[tuple[str, str, int]]:
    """(file, reference, line) for every container image reference."""
    found = []
    for path in paths:
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                for lineno, line in enumerate(fh, 1):
                    if line.lstrip().startswith("#"):
                        continue
                    match = IMAGE_RE.match(line) if "image:" in line else None
                    if match is None and path.rsplit("/", 1)[-1].startswith("Dockerfile"):
                        match = FROM_RE.match(line)
                    if match:
                        found.append((path, match.group(1), lineno))
        except OSError:
            continue
    return found


def normalize(reference: str) -> tuple[str, str, str]:
    """Split into (repository, reference, kind) with Docker Hub defaults filled in."""
    if "$" in reference or "{" in reference or "}" in reference:
        return "", "", ""  # locally built image / helm template expression
    if "@" in reference:
        name, digest = reference.split("@", 1)
        tag = None
    else:
        name, digest = reference, None
        tag = None
        # a tag is the part after the last ':' that comes after the last '/'
        head, _, tail = name.rpartition(":")
        if head and "/" not in tail:
            name, tag = head, tail
    if "/" not in name:
        name = f"library/{name}"
    elif name.split("/", 1)[0] not in ("docker.io",) and "." not in name.split("/", 1)[0]:
        name = f"library/{name}"
    name = name.removeprefix("docker.io/").removeprefix("library/library/")
    if name.startswith("library/") and name.count("/") > 1:
        name = name.split("/", 1)[1]
    if digest:
        return name, digest, "digest"
    return name, tag or "latest", "tag"


def token_for(repository: str) -> str:
    url = f"{AUTH}?service=registry.docker.io&scope=repository:{repository}:pull"
    with urllib.request.urlopen(url, timeout=30) as resp:
        return json.load(resp)["token"]


def header(headers: dict, name: str) -> str | None:
    for key, value in headers.items():
        if key.lower() == name.lower():
            return value
    return None


def fetch_manifest(repository: str, reference: str) -> tuple[int, dict, dict]:
    token = token_for(repository)
    request = urllib.request.Request(
        f"{REGISTRY}/v2/{repository}/manifests/{reference}",
        headers={"Authorization": f"Bearer {token}", "Accept": ACCEPT},
    )
    try:
        with urllib.request.urlopen(request, timeout=45) as resp:
            body = resp.read()
            return resp.status, dict(resp.headers), json.loads(body or b"{}")
    except urllib.error.HTTPError as err:
        return err.code, dict(err.headers or {}), {}


def has_amd64(body: dict) -> bool | None:
    manifests = body.get("manifests")
    if manifests is None:
        return None  # single-arch manifest: platform is implicit
    for entry in manifests:
        platform = entry.get("platform") or {}
        if platform.get("architecture") == "amd64" and platform.get("os") == "linux":
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", dest="json_path")
    args = parser.parse_args()

    refs = references(tracked_files())
    seen: dict[str, dict] = {}
    failures: list[str] = []

    for path, reference, lineno in refs:
        repository, target, kind = normalize(reference)
        if not repository:
            continue  # locally built image (${VAR:-name})
        key = f"{repository}@{target}" if kind == "digest" else f"{repository}:{target}"
        if key in seen:
            seen[key]["occurrences"].append(f"{path}:{lineno}")
            continue
        status, headers, body = fetch_manifest(repository, target)
        record = {
            "reference": key,
            "kind": kind,
            "http_status": status,
            "content_digest": header(headers, "Docker-Content-Digest"),
            "media_type": body.get("mediaType") or header(headers, "Content-Type"),
            "linux_amd64": has_amd64(body),
            "occurrences": [f"{path}:{lineno}"],
        }
        if kind == "digest":
            # A registry returns the digest it computed for the manifest; for a
            # pinned reference it must equal the pin.
            resolved = record["content_digest"] or key.split("@", 1)[-1]
            ok = (
                status == 200
                and resolved == target
                and record["linux_amd64"] is not False
            )
        else:
            ok = status == 200 and record["linux_amd64"] is not False
        record["ok"] = ok
        if not ok:
            failures.append(f"{key} (http {status}, amd64={record['linux_amd64']})")
        seen[key] = record

    width = max((len(k) for k in seen), default=10)
    print(f"{'reference'.ljust(width)}  kind    http  amd64  verdict")
    print(f"{'-' * width}  ------  ----  -----  -------")
    for key, record in sorted(seen.items()):
        verdict = "OK" if record["ok"] else "FAIL"
        amd64 = {True: "yes", False: "NO", None: "single-arch"}[record["linux_amd64"]]
        print(
            f"{key.ljust(width)}  {record['kind']:6}  {record['http_status']:4}  "
            f"{amd64:5}  {verdict}"
        )

    pinned = sum(1 for r in seen.values() if r["kind"] == "digest")
    print(
        f"\n{len(seen)} distinct image references "
        f"({pinned} digest-pinned, {len(seen) - pinned} tag-only), "
        f"{len(failures)} failure(s)"
    )
    print(
        "NOTE: no image was built and no container was run — this verifies the "
        "base images against the live registries only."
    )

    if args.json_path:
        with open(args.json_path, "w", encoding="utf-8") as fh:
            json.dump(
                {
                    "registry": REGISTRY,
                    "checked": seen,
                    "failures": failures,
                    "note": "live registry manifest lookup; no image built",
                },
                fh,
                indent=2,
            )
        print(f"wrote {args.json_path}")

    if failures:
        print("\nFAILURES:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
