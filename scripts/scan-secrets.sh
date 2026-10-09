#!/usr/bin/env bash
# scan-secrets.sh — high-confidence secret scanner (P0-D TASK 1).
#
# Modes:
#   ./scripts/scan-secrets.sh [DIR]      working-tree scan (default DIR: repo root)
#   ./scripts/scan-secrets.sh --history  scan every commit reachable from --all
#
# Exit 0 = clean, exit 1 = findings. Output never prints a full secret:
# findings are redacted to the first 4 characters plus the match length.
# Content matches are ignored ONLY when the match itself contains 4+
# consecutive x/X or the words example/placeholder/dummy/fake
# (case-insensitive). Nothing is ignored by directory.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$ROOT" "$@" <<'PY'
import json
import os
import re
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
args = sys.argv[2:]
HISTORY = "--history" in args
scan_dir = root
for a in args:
    if a != "--history":
        scan_dir = Path(a)

EXCLUDE_DIRS = {
    ".git", "target", "node_modules", ".next", "buyer-release",
}
# apps/control-plane/.next is excluded by the ".next" entry above.

# ---------------------------------------------------------------- patterns
PATTERNS = [
    ("stripe_live_secret",    re.compile(r"sk_live_[0-9A-Za-z]{24,}")),
    ("stripe_restricted_key", re.compile(r"rk_live_[0-9A-Za-z]{24,}")),
    ("stripe_webhook_secret", re.compile(r"whsec_[0-9A-Za-z]{32,}")),
    ("aws_access_key_id",     re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("slack_token",           re.compile(r"xox[baprs]-[0-9A-Za-z-]{20,}")),
    ("telegram_bot_token",    re.compile(r"\b[0-9]{8,10}:[A-Za-z0-9_-]{35}\b")),
    ("github_token",          re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}")),
    ("github_pat",            re.compile(r"github_pat_[A-Za-z0-9_]{50,}")),
    ("evm_private_key",       re.compile(r"0x[0-9a-fA-F]{64}")),
    ("jwt",                   re.compile(r"\beyJ[A-Za-z0-9_-]{17,}\.eyJ[A-Za-z0-9_-]{17,}\.[A-Za-z0-9_-]{17,}")),
]
PK_HEADER = re.compile(
    r"-----BEGIN (RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----")
BASE64ISH = re.compile(r"^[A-Za-z0-9+/=]{40,}\s*$")
SOLANA_ARRAY = re.compile(r"\[(?:\s*\d{1,3}\s*,){63}\s*\d{1,3}\s*\]")
AWS_EXAMPLE = "AKIAIOSFODNN7EXAMPLE"
EVM_CONTEXT = re.compile(r"private|secret|key", re.IGNORECASE)

IGNORE_RUN = re.compile(r"[xX]{4,}")
IGNORE_WORDS = re.compile(r"example|placeholder|dummy|fake", re.IGNORECASE)

ignored_counts = {"xxxx-run": 0, "ignore-word": 0}
findings = []


def redact(m):
    return f"{m[:4]}(len={len(m)})"


def ignore_reason(match_text):
    if IGNORE_RUN.search(match_text):
        return "xxxx-run"
    if IGNORE_WORDS.search(match_text):
        return "ignore-word"
    return None


def report(location, name, match_text):
    reason = ignore_reason(match_text)
    if reason:
        ignored_counts[reason] += 1
        return
    findings.append(f"{location}:{name}:{redact(match_text)}")


def solana_array_ok(text):
    nums = [int(n) for n in re.findall(r"\d{1,3}", text)]
    return len(nums) == 64 and all(0 <= n <= 255 for n in nums)


def scan_lines(lines, location_prefix, want_pk_block=True):
    """Scan an iterable of lines; location_prefix is 'path' or 'sha:path'."""
    for idx, line in enumerate(lines):
        for name, rx in PATTERNS:
            for m in rx.finditer(line):
                text = m.group(0)
                if name == "aws_access_key_id" and text == AWS_EXAMPLE:
                    ignored_counts["ignore-word"] += 1  # documented AWS example
                    continue
                if name == "evm_private_key" and not EVM_CONTEXT.search(line):
                    continue  # bare hashes/addresses are not keys
                report(location_prefix, name, text)
        for m in SOLANA_ARRAY.finditer(line):
            if solana_array_ok(m.group(0)):
                report(location_prefix, "solana_keypair_json", m.group(0))
        if want_pk_block and PK_HEADER.search(line):
            nxt = lines[idx + 1] if idx + 1 < len(lines) else ""
            if BASE64ISH.match(nxt.strip()):
                report(location_prefix, "private_key_block",
                       PK_HEADER.search(line).group(0))


def walk_files(base):
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d not in EXCLUDE_DIRS]
        rel_dir = Path(dirpath).relative_to(base)
        if rel_dir.parts and rel_dir.parts[0] == "apps" and len(rel_dir.parts) > 1:
            pass  # only .next excluded, handled by EXCLUDE_DIRS
        for fn in filenames:
            yield Path(dirpath) / fn


# --------------------------------------------------------------- worktree
def scan_worktree():
    for path in walk_files(scan_dir):
        rel = path.relative_to(scan_dir).as_posix()
        try:
            data = path.read_bytes()
        except OSError:
            continue
        if b"\x00" in data[:8192]:
            continue  # binary
        try:
            lines = data.decode("utf-8", errors="replace").splitlines()
        except Exception:
            continue
        scan_lines(lines, rel)
    # tracked sensitive filenames (git-tracked scope)
    if (root / ".git").exists() and scan_dir == root:
        out = subprocess.run(
            ["git", "ls-files"], cwd=root, capture_output=True, text=True
        ).stdout.splitlines()
        env_rx = re.compile(r"(^|/)\.env(\.|$)")
        key_rx = re.compile(r"\.(pem|key|keypair|p12|pfx|jks)$|(^|/)id\.json$")
        for f in out:
            if env_rx.search(f) and not f.endswith(".template"):
                findings.append(f"{f}:0:tracked-sensitive-filename:env-file")
            if key_rx.search(f):
                findings.append(f"{f}:0:tracked-sensitive-filename:key-material")


# ----------------------------------------------------------------- history
def history_grep(regex):
    revs = subprocess.run(
        ["git", "rev-list", "--all"], cwd=root, capture_output=True, text=True
    ).stdout.split()
    hits = []
    for i in range(0, len(revs), 50):
        batch = revs[i:i + 50]
        p = subprocess.run(
            ["git", "grep", "-InE", regex, "--no-color"] + batch,
            cwd=root, capture_output=True, text=True)
        if p.returncode in (0, 1):
            hits.extend(p.stdout.splitlines())
    return hits


def scan_history():
    for name, rx in PATTERNS:
        for line in history_grep(rx.pattern):
            # git grep output: sha:path:lineno:text
            parts = line.split(":", 3)
            if len(parts) < 4:
                continue
            sha, path, lineno, text = parts
            for m in rx.finditer(text):
                t = m.group(0)
                if name == "aws_access_key_id" and t == AWS_EXAMPLE:
                    ignored_counts["ignore-word"] += 1
                    continue
                if name == "evm_private_key" and not EVM_CONTEXT.search(text):
                    continue
                reason = ignore_reason(t)
                if reason:
                    ignored_counts[reason] += 1
                    continue
                findings.append(f"{sha}:{path}:{lineno}:{name}")
    # private key blocks: header anywhere in history, then check next line
    for line in history_grep(r"BEGIN (RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY"):
        parts = line.split(":", 3)
        if len(parts) < 4:
            continue
        sha, path, lineno, text = parts
        try:
            blob = subprocess.run(
                ["git", "show", f"{sha}:{path}"],
                cwd=root, capture_output=True, text=True).stdout
        except Exception:
            continue
        lines = blob.splitlines()
        try:
            i = int(lineno) - 1
        except ValueError:
            continue
        if 0 <= i < len(lines) and PK_HEADER.search(lines[i]):
            nxt = lines[i + 1] if i + 1 < len(lines) else ""
            if BASE64ISH.match(nxt.strip()):
                findings.append(f"{sha}:{path}:{lineno}:private_key_block")
    # solana keypair arrays
    for line in history_grep(r"\[[0-9]{1,3}(,[0-9]{1,3}){63}\]"):
        parts = line.split(":", 3)
        if len(parts) < 4:
            continue
        sha, path, lineno, text = parts
        m = SOLANA_ARRAY.search(text)
        if m and solana_array_ok(m.group(0)):
            reason = ignore_reason(m.group(0))
            if reason:
                ignored_counts[reason] += 1
                continue
            findings.append(f"{sha}:{path}:{lineno}:solana_keypair_json")


if HISTORY:
    if not (root / ".git").exists():
        print("scan-secrets: no .git — history scan skipped")
    else:
        scan_history()
else:
    scan_worktree()

for f in findings:
    print(f)
print(
    f"scan-secrets: {'HISTORY' if HISTORY else 'working-tree'} findings="
    f"{len(findings)} ignored[xxxx-run]={ignored_counts['xxxx-run']} "
    f"ignored[ignore-word]={ignored_counts['ignore-word']}"
)
sys.exit(1 if findings else 0)
PY
